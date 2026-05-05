# AST-body audit tools — Phase 8 plan

Five new MCP tools that close the **fn-body discipline gap** in the rust-guidelines audit coverage. Today our workflows enforce ~65-70% of `rust-guidelines-final.md`; the remaining ~30% is body-level idiom checks (no `unwrap()` in production paths, no `.await` while holding a lock, recursion in critical paths, etc.) that need AST inspection inside fn bodies.

All five tools fit the existing query-time audit pattern (mirror `unsafe_audit` from Phase 6 and `mut_static_audit` from Phase 7 Path B). **No schema bumps** — every tool reads from the existing v11 snapshot or walks the workspace AST via `loader::load`.

After Phase 8 ships, guideline coverage rises to ~95% — strong enough to drive a `/guidelines-audit` skill and a CI checklist.

---

## Background

### What's missing today

The workflows doc (`workflows-detailed.md` W1-W24) and tool surface (TOOLS.md, 42 tools) cover architecture, complexity, unsafe surface, dependency hygiene, and duplicate detection well. They **don't** cover:

| Guideline | What's missing |
|---|---|
| §3 Readability — no `_ =>` in match, no `unwrap` after boolean test | fn-body pattern recognition |
| §9 Errors — no `unwrap()` / `expect()` in production paths, no panic in `Drop::drop` | call-site walking inside fn bodies |
| §12 Async boundary — no lock guards held across `.await` | scope-aware analysis inside async bodies |
| §12 Bounded channels — `mpsc::channel(N)` audit | construction-call inspection |
| §16 Documentation — pub items without `///` doc-comments | metadata cross-reference (data already extracted) |
| §8 Standard derive coverage — `Debug` always, `Clone` where meaningful | metadata cross-reference (data already extracted) |
| §22 Safety-critical — recursion in critical paths, unbounded loops | call-graph closure + body-loop inspection |

The unifying capability is **walking the AST inside fn bodies** (or applying simple cross-references over already-extracted metadata).

### What exists to reuse

- **`loader::load(workspace_root)`** — full RA workspace load (~2-3s wall-clock). Already wrapped in `tokio::task::spawn_blocking` by `unsafe_audit` and `build_hypergraph`. Same pattern applies here.
- **`Module::definition_source_file_id(db)`** + AST walk — used by `unsafe_audit` to find `BlockExpr::unsafe_token()`. Mirror for fn-body walking.
- **Phase 5 `signatures_by_target`** — gives us NodeId → fn signature, enabling per-fn iteration without a fresh extraction pass.
- **Phase 1 `node.attributes`** field (v8 schema) — every Item carries its outer attributes and `///` doc-comment lines. Tools 2 and 3 read this directly without touching AST.
- **Layer 10 call graph** (`callees_by_caller` / `callers_by_callee`) — Tool 5 walks this for cycle detection.
- **`ra_ap_syntax::ast::*`** — `MethodCallExpr`, `MacroCall`, `AwaitExpr`, `LoopExpr`, `LetStmt`, `CallExpr`, `PathExpr` — the building blocks for body-walk patterns.

---

## The five tools

### Tool 1 — `fn_body_audit` (~450 LOC)

The most involved tool. Walks fn bodies and reports per-pattern violations.

**Tool surface:**

```rust
pub struct FnBodyAuditParams {
    pub directory: String,
    /// Optional crate qualified name to scope the scan. Default: all local crates.
    pub crate_name: Option<String>,
    /// Patterns to check. Default: all built-in patterns.
    /// Built-ins: "unwrap", "expect", "panic_macros", "await_in_guard_scope",
    ///            "self_recursion", "unbounded_loop", "unwrap_unchecked", "transmute".
    pub patterns: Option<Vec<String>>,
    /// Skip findings inside `#[cfg(test)]` modules / fns. Default true.
    pub skip_test_fns: Option<bool>,
}

pub struct FnBodyFinding {
    pub target: String,           // NodeId hex of the enclosing fn
    pub qualified_name: String,
    pub pattern: String,
    pub file: String,
    pub span: (u32, u32),
    pub context: String,          // 1-3 lines of source around the hit
}

pub struct FnBodyAuditResp {
    pub scope: ScopeSummary,
    pub patterns_used: Vec<String>,
    pub finding_count: usize,
    pub findings: Vec<FnBodyFinding>,
}
```

**Patterns shipped in v1:**

| Pattern name | What it matches | Guideline |
|---|---|---|
| `unwrap` | `MethodCallExpr` where method name is `unwrap` (Result/Option) | §9 — "Avoid `unwrap()` in production paths" |
| `expect` | `MethodCallExpr` where method name is `expect` | §9 — "Avoid `expect()` in library code except for locally provable invariants" |
| `panic_macros` | `MacroCall` with name in {`panic`, `unreachable`, `todo`, `unimplemented`} | §9 — "Use `panic!` for bugs only" |
| `unwrap_unchecked` | `MethodCallExpr` for `unwrap_unchecked` / `unwrap_err_unchecked` | §19 — "Treat every unsafe change as security-sensitive" |
| `transmute` | `CallExpr` resolving to `std::mem::transmute` (path match `mem::transmute` or `core::mem::transmute`) | §19 — "Use `unsafe` only when safe Rust cannot express the operation" |
| `await_in_guard_scope` | `AwaitExpr` whose nearest enclosing block contains an in-scope `LetStmt` binding a guard type (`MutexGuard`, `RwLockReadGuard`, `RwLockWriteGuard`, `parking_lot::*Guard`, `Ref`, `RefMut`) | §12 — "Never hold a lock or span guard across `.await`" |
| `self_recursion` | fn body contains a `CallExpr` resolving to the enclosing fn's NodeId | §22 — "No recursion in critical paths" |
| `unbounded_loop` | `LoopExpr` (raw `loop { ... }`) with no `break` / `return` statement at any depth — heuristic | §22 — "Give loops clear upper bounds when practical" |

**Algorithm:**

1. Spawn-blocking the whole audit (mirrors `unsafe_audit`). Inside:
   - `loader::load(canonical)` — fresh RA load.
   - Open the v11 snapshot read-only.
2. Iterate every fn NodeId in the snapshot's `signatures_by_target` (Phase 5 has them indexed). For each:
   - Resolve to the RA `Function` semantic def via `(file, span)` + module walk.
   - Walk the fn body's syntax tree.
   - For each enabled pattern, run the matcher.
   - Emit findings.
3. `skip_test_fns=true` (default): skip fns whose qualified_name contains `::tests::` or whose enclosing module has `#[cfg(test)]`.
4. Return findings sorted by `(file, span.0)`.

**Scope-aware checks (the hard part):**

- `await_in_guard_scope`: walk every `AwaitExpr` in the fn body. For each, climb ancestors until we hit a `BlockExpr`. Within that block, scan all `LetStmt`s that come *before* the `.await` location. If any `LetStmt`'s type ascription or initializer expression mentions a guard type (string-match on `MutexGuard` / `RwLockReadGuard` / `RwLockWriteGuard` / `Guard` / `Ref` / `RefMut`), flag it. False positives are acceptable; this is a review trigger.
- `self_recursion`: get the enclosing fn's NodeId. Walk all `PathExpr` and `CallExpr` in the body. Use RA's `Semantics::resolve_path` to map each to a NodeId. If any matches self, flag.
- `unbounded_loop`: walk every `LoopExpr` (the `loop` keyword form, not `for`/`while`). Walk its body. If we find no `BreakExpr` / `ReturnExpr` / `?` operator anywhere in the descendants, flag. Heuristic — event loops will trigger; document this.

**Cost:** workspace fn count × constant body-walk overhead. Rough estimate: a 2000-fn workspace with ~50 lines avg body = 100k AST nodes to walk × O(1) per pattern = ~5-10s wall-clock. Wrap in spawn_blocking; one call doesn't block the runtime worker.

**Tests:**

- Per-pattern unit tests using small AST snippets:
  - `fn x() { x.unwrap() }` → unwrap pattern fires.
  - `let _g = mutex.lock().unwrap(); foo().await; drop(_g);` → await_in_guard_scope fires.
  - `fn x() { x() }` → self_recursion fires.
  - `loop { println!("hi"); }` → unbounded_loop fires.
  - `loop { if cond { break; } }` → unbounded_loop does NOT fire.
- Each matcher is a free function over `&SyntaxNode` so unit tests are straightforward.

### Tool 2 — `derive_audit` (~150 LOC)

Find Items missing expected derive macros. Pure read-side query on `node.attributes` (Phase 1 already extracted these). No AST walk needed.

**Tool surface:**

```rust
pub struct DeriveAuditParams {
    pub directory: String,
    pub crate_name: Option<String>,
    /// Item kind to audit: "Struct" | "Enum" | "Union". Default: all three.
    pub item_kind: Option<String>,
    /// Required derives (e.g. ["Debug"] or ["Debug", "Clone", "PartialEq"]).
    /// At minimum, `Debug` is the canonical recommendation.
    pub required_derives: Vec<String>,
    /// Only audit items whose visibility is `pub` (the rule "Debug almost always"
    /// applies to the public surface). Default true.
    pub pub_only: Option<bool>,
}

pub struct DeriveFinding {
    pub target: String,           // NodeId hex
    pub qualified_name: String,
    pub item_kind: String,
    pub file: String,
    pub span: (u32, u32),
    pub current_derives: Vec<String>,
    pub missing_derives: Vec<String>,
}
```

**Algorithm:**

1. Iterate `nodes_by_id` filtered to NodeKind::Item with `item_kind ∈ {Struct, Enum, Union}` (or per param) and `crate_id` filter.
2. If `pub_only=true`: filter `node.visibility == "pub"`.
3. For each, parse `node.attributes` to extract derive contents:
   - For each attribute string starting with `#[derive(`, parse the parenthesized list.
   - Trivia-tolerant parser: split on `,`, strip whitespace, keep only the leading identifier (handles `serde::Serialize` → `Serialize`).
4. Compare against `required_derives`. Emit a finding if any required derive is missing.

**No AST walk needed** — `node.attributes` already contains the rendered attribute text. ~30s for an 8000-item workspace, but pure heap iteration.

**Tests:**

- `#[derive(Debug)]` matches required `["Debug"]`.
- `#[derive(Debug, Clone, PartialEq)]` matches required `["Debug", "Clone"]`.
- `#[derive(serde::Serialize)]` matches required `["Serialize"]`.
- Item with no `#[derive(...)]` reports all required as missing.

### Tool 3 — `missing_docs_audit` (~80 LOC)

Find pub items without doc-comments. Simplest tool. Pure read-side query.

**Tool surface:**

```rust
pub struct MissingDocsAuditParams {
    pub directory: String,
    pub crate_name: Option<String>,
    /// Item kinds to audit. Default: all "documentable" kinds (excludes
    /// EnumVariant, AssocConst, AssocType to reduce noise — those rarely
    /// carry standalone docs).
    pub item_kind: Option<Vec<String>>,
    /// Drop items inside `::tests::` modules. Default true.
    pub skip_test_items: Option<bool>,
}

pub struct MissingDocsFinding {
    pub target: String,           // NodeId hex
    pub qualified_name: String,
    pub item_kind: String,
    pub visibility: String,
    pub file: String,
    pub span: (u32, u32),
}
```

**Algorithm:**

1. Iterate `nodes_by_id` filtered to NodeKind::Item, `item_kind` filter, `crate_id` filter.
2. Keep only items where `node.visibility == "pub"`.
3. Skip if `skip_test_items=true` and qualified_name contains `::tests::`.
4. Check `node.attributes` for any line starting with `///`. If none, emit finding.
5. Sort by `(crate, file, span.0)`.

**Tests:**

- Item with `["/// docs"]` → no finding.
- Item with no doc-comment lines → finding.
- Item with only `#[derive(Debug)]` (no `///`) → finding.

### Tool 4 — `channel_capacity_audit` (~300 LOC)

Find every channel-construction call site in the workspace. AST-walk tool.

**Tool surface:**

```rust
pub struct ChannelCapacityAuditParams {
    pub directory: String,
    pub crate_name: Option<String>,
    /// Drop findings inside `#[cfg(test)]` modules / fns. Default true.
    pub skip_test_fns: Option<bool>,
}

pub struct ChannelFinding {
    pub crate_name: String,
    pub kind: String,                  // "tokio_mpsc" | "tokio_unbounded" | "std_mpsc"
                                       // | "std_sync_channel" | "crossbeam_bounded"
                                       // | "crossbeam_unbounded" | "flume_bounded"
                                       // | "flume_unbounded"
    pub bounded: bool,
    pub capacity: Option<u64>,         // Some(N) for bounded literal, None for non-literal expr or unbounded
    pub file: String,
    pub span: (u32, u32),
    pub enclosing_function: Option<String>,
}
```

**Path patterns matched:**

| Path | Bounded? | Notes |
|---|---|---|
| `tokio::sync::mpsc::channel(N)` | yes | Default tokio bounded mpsc |
| `tokio::sync::mpsc::unbounded_channel()` | no | Tokio unbounded — flag |
| `std::sync::mpsc::channel()` | no | Legacy std unbounded — flag |
| `std::sync::mpsc::sync_channel(N)` | yes | Std bounded |
| `crossbeam_channel::bounded(N)` | yes | |
| `crossbeam_channel::unbounded()` | no | |
| `flume::bounded(N)` | yes | |
| `flume::unbounded()` | no | |

**Algorithm:**

1. Spawn-blocking + `loader::load`.
2. Walk each crate's modules' source files.
3. For every `CallExpr`, resolve the call's path. RA's `Semantics::resolve_path` gives us the canonical fully-qualified path even when the source has a shortened import (e.g. `mpsc::channel`).
4. Match the canonical path against the known patterns.
5. For bounded channels, attempt to extract the literal capacity argument. If non-literal (a const, a fn call, etc.), emit `capacity: None` with `bounded: true`.
6. Find enclosing fn via `Semantics::ancestors_with_macros`.
7. `skip_test_fns=true` (default): skip if enclosing module has `#[cfg(test)]`.

**Tests:**

- `tokio::sync::mpsc::channel(100)` → tokio_mpsc, bounded=true, capacity=Some(100).
- `tokio::sync::mpsc::unbounded_channel()` → tokio_unbounded, bounded=false.
- `mpsc::channel(BUF_SIZE)` (where BUF_SIZE is const) → tokio_mpsc, bounded=true, capacity=None.
- Path resolution edge case: `use tokio::sync::mpsc; mpsc::channel(10)` should still match.

### Tool 5 — `recursion_check` (~120 LOC)

Find fns that participate in a recursion cycle. Pure read-side query on Layer 10 call graph data.

**Tool surface:**

```rust
pub struct RecursionCheckParams {
    pub directory: String,
    pub crate_name: Option<String>,
    /// Maximum cycle length to detect. Default: 5 (covers self-loop +
    /// indirect recursion through a few levels). Cap: 12.
    pub max_cycle_length: Option<usize>,
}

pub struct RecursionCycle {
    pub fns: Vec<String>,             // Qualified names in cycle order
    pub cycle_length: usize,
    pub direct_recursion: bool,       // true if length=1 (fn calls itself)
    pub starting_node_id: String,     // hex NodeId of the cycle's lowest-id member
}

pub struct RecursionCheckResp {
    pub scope: ScopeSummary,
    pub max_cycle_length: usize,
    pub cycle_count: usize,
    pub cycles: Vec<RecursionCycle>,
}
```

**Algorithm:**

1. For each fn NodeId in `signatures_by_target` (filtered to crate if requested):
   - Run a bounded DFS via `callees_by_caller` (or whichever sub-DB Layer 10 added).
   - Track the visit path. If the DFS reaches the starting node, we've found a cycle.
   - Cap depth at `max_cycle_length`.
2. Dedupe cycles by their canonical form (rotate so the lowest-id NodeId comes first).
3. Sort by `(cycle_length asc, qualified_name)`.
4. `direct_recursion = (cycle_length == 1)` — flag self-recursion separately for the §22 audit.

**Cost:** N fns × O(D × edges) where D is depth cap. For 2000 fns and depth 5, ~10ms. Fast.

**Tests:**

- `fn a() { a() }` → length-1 cycle.
- `fn a() { b() } fn b() { a() }` → length-2 cycle.
- Mutual recursion across crates: dedup correctly.
- No cycles in a clean codebase → empty.

---

## Implementation order

1. **Tool 3 (`missing_docs_audit`)** — easiest. Validates the "iterate nodes_by_id with filters" pattern + the `node.attributes` reading approach. ~80 LOC.
2. **Tool 2 (`derive_audit`)** — extends Tool 3 with attribute parsing. ~150 LOC. Reuses the Tool 3 iteration scaffold.
3. **Tool 5 (`recursion_check`)** — pure read-side query, but exercises Layer 10 traversal in a new way. ~120 LOC. Independent of Tools 1-4 (different data).
4. **Tool 4 (`channel_capacity_audit`)** — first AST-walk tool. Establishes the `loader::load` + path-resolution pattern that Tool 1 builds on. ~300 LOC.
5. **Tool 1 (`fn_body_audit`)** — most invasive. Builds on Tool 4's AST scaffolding and adds 8 pattern matchers + scope-aware checks. ~450 LOC.

**Total:** ~1100 LOC of tool code + ~200 LOC of param structs / router glue / tests = **~1300 LOC, no schema bumps**.

Ship in order; each tool is independently usable and validates the next layer of complexity.

---

## File layout

| Tool | Module | Wrapper |
|---|---|---|
| 1 | `src/graph/fn_body_audit.rs` | `src/tools/graph_tools.rs::fn_body_audit` |
| 2 | `src/graph/derive_audit.rs` | `src/tools/graph_tools.rs::derive_audit` |
| 3 | `src/graph/docs_audit.rs` | `src/tools/graph_tools.rs::missing_docs_audit` |
| 4 | `src/graph/channel_audit.rs` | `src/tools/graph_tools.rs::channel_capacity_audit` |
| 5 | `src/graph/recursion_check.rs` | `src/tools/graph_tools.rs::recursion_check` |

All five MCP routes added to `src/tools/search_tool_router.rs`. Param structs added to `src/tools/search_tool.rs`. TOOLS.md gets an entry per tool plus the v1.x version notes.

---

## Risks

| Risk | Mitigation |
|---|---|
| Tool 1's `await_in_guard_scope` has false positives (e.g. `let _g = compute_guard()` where the type isn't actually a lock) | Document as a review trigger, not a hard error. Pattern is opt-in via `patterns` parameter. |
| Tool 1's `unbounded_loop` flags legitimate event loops | Document; let users disable the pattern for files that intentionally use `loop {}` (event loops, server accept loops). Ship with the pattern enabled by default since it surfaces real §22 violations. |
| Tool 4's path resolution may miss when source uses a `use` alias (`use tokio::sync::mpsc as mp;`) | RA's `Semantics::resolve_path` follows aliases. Verified against `unsafe_audit`'s similar resolution. |
| Tool 5's mutual recursion across crates explodes for deep call graphs | `max_cycle_length` cap (default 5, hard max 12); BFS with visited-set dedupes. |
| Tool 1's wall-clock cost (~5-10s on big workspaces) | Pattern selection (`patterns` param) lets users disable expensive checks. spawn_blocking keeps the runtime free. |
| `node.attributes` doesn't always include doc-comments for macro-generated items | Same caveat as Phase 1; the Tool 3 result is "extractable" docs, not "should have docs". Document. |
| derive parsing (Tool 2) trips on `#[derive(serde::Serialize)]` vs `#[derive(Serialize)]` | Strip leading path segments — keep only the final identifier. `Serialize` and `serde::Serialize` both match `Serialize` in `required_derives`. |
| Tool 1 missing the case where `unwrap` is method-named on a non-Result/Option type | Accept false positives for v1; the pattern is `MethodCallExpr` named `unwrap`. Type-aware filtering is a v2 enhancement. |

---

## Decision points (resolve before implementing)

1. **Default pattern set for Tool 1**: ship with all 8 enabled, or only the "high-signal" subset? **Recommend**: all 8 enabled by default; users opt out via `patterns: ["unwrap", "expect"]` to narrow. Rationale: the cost is the same regardless of pattern count (the body walk is the dominant cost); users who don't want noise will use the `patterns` filter anyway.

2. **`pub_only` default for Tool 2 (`derive_audit`)**: `true` (audit only public surface) or `false` (audit everything)? **Recommend**: `true`. The "Debug almost always" rule applies to the public API; private types where Debug isn't needed shouldn't show up as findings. Users can flip for thorough audits.

3. **Skip-test default**: every tool has `skip_test_fns` / `skip_test_items` defaulting to `true`. **Recommend**: keep at `true`. Test fixtures and `#[cfg(test)]` items dominate noise; users flip when explicitly auditing test code.

4. **Tool 4 path-pattern table**: hardcoded vs configurable? **Recommend**: hardcoded for v1. Adding new channel libraries (e.g. `kanal`, `concurrent_queue`) is a tool update, not a per-call config. v2 may expose a `extra_patterns` param.

5. **Tool 1 enclosing-fn attribution for nested closures**: when an `unwrap()` is inside a closure inside a fn, attribute to the outer fn? **Recommend**: yes — closures don't get their own NodeId in the snapshot. Mirror `unsafe_audit`'s "enclosing fn" logic.

---

## What NOT to build (yet)

- **Type-aware `unwrap` detection** (only flag `Result::unwrap` / `Option::unwrap`, not user-defined methods named `unwrap`). v2 enhancement; needs RA's `Semantics::type_of_expr`.
- **Lifetime-of-guard analysis** (Tool 1) — fully accurate "is this guard actually held across the await?" requires drop-elaboration analysis. Way out of scope. The string-match heuristic is what ships.
- **Cross-tool synthesis** (e.g. "every fn flagged by `unsafe_audit` AND by `fn_body_audit::unwrap`"). Build via composition in skills, not a new tool.
- **Custom-pattern DSL for Tool 1** — no `pattern: "MethodCallExpr where name in [foo, bar]"` config language. Hardcoded patterns + maintenance updates.
- **Per-fn caching of Tool 1 findings** — possible (key on fn's content_hash) but premature. The full audit takes <10s; cache complexity isn't worth it yet.
- **Auto-fix suggestions** — these are audit tools, not codemods. Users get findings; humans decide.

---

## Effort summary

| Tool | LOC | AST walk? | New sub-DB? | Cost reduction over manual review |
|---|---|---|---|---|
| Tool 3 — `missing_docs_audit` | ~80 | no (read-only) | no | Eliminates manual `grep -L "///" src/**/*.rs` |
| Tool 2 — `derive_audit` | ~150 | no (read-only) | no | Eliminates per-Item manual `#[derive(...)]` review |
| Tool 5 — `recursion_check` | ~120 | no (Layer 10) | no | Surfaces self-cycles + mutual recursion in one call |
| Tool 4 — `channel_capacity_audit` | ~300 | yes | no | Workspace-wide channel-construction inventory |
| Tool 1 — `fn_body_audit` | ~450 | yes | no | The big one — closes ~30% of guideline coverage |
| **Total** | **~1100** | **2 walking** | **0 schema bumps** | **~95% guideline coverage** |

Plus ~200 LOC for param structs / router wiring / unit tests.

---

## After Phase 8 — what changes

Once all five tools land:

- **`/guidelines-audit` skill** becomes feasible. Runs all five new tools + the existing `unsafe_audit` / `mut_static_audit` / `dead_pub_report` / `forbidden_dependency_check` / `analyze_complexity`, cross-references against `rust-guidelines-final.md` sections, and emits a per-section pass/fail report with file:span citations.
- **CI integration**: every tool's findings can be enforced in CI as either a block or a warning. The natural format is one job per tool with a sortable JSON output.
- **Refactor planner subagent** gains visibility into body-level violations, not just structural ones. A "complex AND uses unwrap heavily" finding becomes possible.
- **Snapshot diffs (W16)** gain a body-level dimension: "this PR added 3 new `unwrap()` calls" / "channel previously bounded(1024) is now unbounded".

Total guideline coverage rises from ~65% to ~95%. The remaining 5% (performance profiling, edition-specific compiler quirks, dependency supply chain) are genuinely outside the graph's scope and best handled by external tools (`cargo-bench`, `cargo-deny`, etc.).

---

## Sources

- `.docs/rust-guidelines-final.md` — guideline source of truth.
- `.docs/workflows-detailed.md` W20 (`unsafe_audit`) — reference implementation for AST-walk tools.
- `.docs/workflows-detailed.md` W21 (`mut_static_audit`) — reference implementation for type-aware audits.
- `src/graph/unsafe_audit.rs` — Phase 6 module; the closest existing analog to Tool 1.
- `src/graph/statics.rs` — Phase 7 Path B build-time extraction; reference for Tool 4's path-resolution.
- `src/graph/queries.rs::functions_with_filter` — reference for Tools 2/3/5's per-crate iteration.
- `ra_ap_syntax::ast` — AST node types: `MethodCallExpr`, `MacroCall`, `AwaitExpr`, `LoopExpr`, `LetStmt`, `CallExpr`, `PathExpr`.
- `ra_ap_hir::Semantics::resolve_path` — canonical path resolution (used in Tool 4).
