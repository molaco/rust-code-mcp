# G11 — AST-body audits review

Commits reviewed (chronological):

- `d6cfed32` — `[review] add plan document for ast body tools` (427 LOC, `.plans/ast-body-tools.md`)
- `758a2c7d` — `[review] add channel/derive/docs/fn-body/recursion audit modules and wire them into graph tools` (3,247 LOC)
- `2e2e7fb3` — `[review] add ast_resolve helper for turbofish-safe call resolution in channel and fn body audits` (119 LOC)

## 1. Group summary

Phase 8 of the project: five new MCP audit tools (`missing_docs_audit`, `derive_audit`, `recursion_check`, `channel_capacity_audit`, `fn_body_audit`) that close most of the body-level "rust-guidelines" coverage gap. Three are pure read-side queries against the v11 snapshot (docs / derive / recursion) and two are AST-walks gated by a fresh `loader::load` (channel / fn_body). The implementation maps cleanly to the plan (Phase 8 plan in `d6cfed32` predicted ~1100 LOC + ~200 LOC glue; the actual feature commit lands at ~3.2 KLOC with the bulk of the overage coming from sizeable per-module unit-test suites, extended tool descriptions, and the `graph_tools.rs`/`search_tool.rs`/router glue). The follow-up `2e2e7fb3` retroactively patches a real bug: `Semantics::resolve_path` returns `None` for paths carrying a turbofish (`foo::<T>()`), which would silently swallow channel / transmute / self-recursion matches. The fix introduces a small shared helper (`graph/ast_resolve.rs`) that routes through `resolve_expr_as_callable` and applies it at the two known call sites. The build is clean (`cargo check --lib` produces only pre-existing warnings).

## 2. Per-commit review

### 2.1 `d6cfed32` — plan document (~427 LOC)

**What it does:** drops `.plans/ast-body-tools.md` describing the five tools' surface (params + response struct), pattern lists, algorithm sketch, cost estimates, risk table, decision points, and explicit "what NOT to build" non-goals. Recommends an implementation order (3 → 2 → 5 → 4 → 1) and effort breakdown.

**Issues:** none material. The plan is unusually thorough; the only minor mismatches with the shipped implementation are noted in §3 ("Plan vs impl fidelity").

**Verdict:** PASS — high-quality design document, nothing to fix.

### 2.2 `758a2c7d` — implementation (3,247 LOC)

Six new files plus router/param glue. Reviewed per module:

#### 2.2.1 `channel_capacity_audit` (`src/graph/channel_audit.rs`, 443 LOC)

**What it does:** AST-walks every local-crate source file, casts each `CallExpr`, requires the callee to be a `PathExpr`, resolves the canonical fully-qualified function path through `Semantics::resolve_path` (later swapped to `resolve_expr_as_callable` in `2e2e7fb3`), and matches against a hardcoded table of 8 paths covering tokio mpsc (bounded + unbounded; also the `bounded::channel`/`unbounded::unbounded_channel` defining-module variants), std mpsc / sync_channel, crossbeam, and flume. For bounded constructors, the first arg is decoded as a literal int (`_` separators tolerated; arithmetic / consts / negative → `None`). Enclosing fn is resolved through `scope_at_offset → containing_function → canonical_function_path` then looked up by qualified name in the snapshot.

**Issues:**

- **(low)** The `local_crate_filter` is built from the resolved `node.qualified_name` for a *single* node lookup, but then matched against each crate's `display_name(db).canonical_name()`. These are generally the same for a top-level crate, but for a Module the crate filter would have to first walk to the crate. The `crate_id_filter` resolution wrapper at the tool layer (`graph_tools.rs::channel_capacity_audit`) coerces Module → Crate via `node.crate_id.or(node.parent_id)`, which mitigates this, but the channel-audit function still de-references the *original* id's `qualified_name`. If a caller bypasses the tool layer and passes a Module id, the filter would silently match against a module-qualified name and never hit any crate. Library-internal only, so not a release blocker.
- **(low)** `parse_capacity_arg` returns `None` for `-1`, which is the correct conservative answer, but `0` is `Some(0)`. The plan flagged that bounded constructors generally reject `0` at runtime (tokio panics); flagging `capacity == Some(0)` as a separate "almost-certainly-a-bug" finding would be an obvious v2 addition.
- **(low)** `enclosed_by_cfg_test` walks ancestors using `ast::Item::cast`; its `item_has_cfg_test` branches over only nine `Item` variants. A `CallExpr` inside a `ConstParam`, a doctest-only `ExternBlock`, or a nested macro body would not be caught — typically irrelevant in practice. Strings like `cfg(any(test, debug_assertions))` are matched by `stripped.contains("cfg(any(test")`, which has a tiny false-positive surface if someone literally wrote `cfg(any(test_only))`. Acceptable.
- **(info)** The classification table accepts the `tokio::sync::mpsc::bounded::channel` / `tokio::sync::mpsc::unbounded::unbounded_channel` paths, presumably because RA resolves through tokio's `pub use` to the defining sub-module. Good defensive coverage; explicit unit tests confirm both.
- **(info)** No `kanal`, no `tokio::sync::oneshot` (which is bounded-1 implicitly), no `tokio::sync::broadcast`. The plan called this out as v1 hardcoded and v2-configurable; not a defect.

**Tests:** 19 unit tests covering the path-classification table and the literal-capacity parser. No integration test exercising the full `loader::load` + AST walk, but that's consistent with the rest of the workspace (test_full_incremental_flow.rs etc. are end-to-end; per-tool wiring isn't tested via the MCP boundary).

**Verdict:** PASS — clean, well-tested module.

#### 2.2.2 `derive_audit` (`src/graph/derive_audit.rs`, 403 LOC)

**What it does:** pure-snapshot read. Walks `nodes_by_id`, filters to `NodeKind::Item` with `item_kind ∈ {Struct, Enum, Union}` (configurable). To get the *effective* visibility (which lives on the declaring Binding, not the Node), it walks `bindings_by_id`, prefers a parent-module-matching `Declared` binding, then keeps Public ones (or, with `pub_only=false`, anything). `extract_derives` parses `#[derive(...)]` attribute strings — including absolute paths (`::std::fmt::Debug`) and serde-style qualifiers (`serde::Serialize`) — and returns the trailing identifier set. `missing_required_derives` returns the set difference.

**Issues:**

- **(low)** The derive parser uses `rfind(")]")` to find the closing pair and trims one trailing `)` after extracting `inner`. The comment claims this handles nested close-parens for `#[derive(Foo)]`. In practice every `#[derive(...)]` is by definition flat (no nesting allowed inside the derive list), but the defensive trim is harmless. The parser **doesn't** handle a derive with arguments (e.g. `#[derive(derivative::Derivative)]` where `Derivative` is itself fine; but if someone wrote `#[derive(Display(custom))]` — not legal Rust today — it'd ignore the inner parens incorrectly). Not a real concern.
- **(low)** A derive that's macro-imported as something Rust-illegal-but-tokenizable (e.g. `#[derive(crate::Foo)]`) yields `Foo` after `rsplit_once("::")`. Required-set match against `"Foo"` works; against `"crate::Foo"` wouldn't. The plan called this out as an intentional v1 stripping rule.
- **(low)** `node.visibility` is populated post-resolution to `"pub"` / `"non-pub"` even though `Item` Nodes don't natively carry visibility — `missing_required_derives` then re-checks `node.visibility.as_deref() == Some("pub")`. This is correct given the call sequence inside `derive_audit`, but the public predicate function `missing_required_derives` requires the caller to remember to pre-populate visibility. The doc comment says so; still, it's a footgun if someone reuses the predicate elsewhere.
- **(info)** `required_derives` empty-check is enforced at the tool boundary (`graph_tools.rs::derive_audit`), not in this module — which is the right place.
- **(info)** No conflict detection (e.g. `Copy` requested while `Drop` is implemented; or `Debug` requested when `#[derive(custom_derive::Debug)]` already provides it). The plan was explicit that this is a v1 set-difference audit; v2 territory.

**Tests:** 13 unit tests covering parser variants (single, multiple, absolute path, qualified path, whitespace, multi-attr accumulation, non-derive attr, doc comment) and the predicate (flags pub-struct missing-clone, skips when all present, skips non-pub, skips tests, skips wrong kind).

**Verdict:** PASS — comprehensive and matches the spec.

#### 2.2.3 `missing_docs_audit` (`src/graph/docs_audit.rs`, 268 LOC)

**What it does:** identical scaffolding to `derive_audit` — walk `nodes_by_id` filtered to Item, build the `bindings_by_id`-based visibility lookup, emit findings for pure-`pub` items whose `node.attributes` contains no line starting with `///`. Default kind set is `{Function, Struct, Enum, Union, Trait, TypeAlias, Const, Static, Method}` (drops `EnumVariant`, `AssocConst`, `AssocType`).

**Issues:**

- **(low)** `is_undocumented_pub_item` uses `a.starts_with("///")` to detect a doc-comment line. A `#[doc = "..."]` attribute (which is the canonical way to attach docs programmatically and is what RA emits for macro-generated docs) is **not** matched. The plan explicitly noted "`node.attributes` doesn't always include doc-comments for macro-generated items" as a known caveat, so this is per design — but the tool's description would benefit from "`#[doc = ...]` attributes are not counted as a doc-comment", which currently isn't called out. **Severity: low — by-design but not user-documented.**
- **(low)** The plan section "Tool 3 — `missing_docs_audit`" did not mention `#[doc(hidden)]`. The implementation also does not skip `#[doc(hidden)]` items — they will be flagged as undocumented even though the convention is they need no docs. **Severity: low — small false-positive surface.**
- **(info)** Items whose declaring Binding wasn't found in `bindings_by_id` are silently dropped (`continue`), even when `pub_only` would be off in a hypothetical extension. This is the correct conservative default.
- **(info)** "Documentable" default kinds intentionally exclude `EnumVariant` / `AssocConst` / `AssocType`. If a user wants those, they can opt in via `item_kind`.

**Tests:** 4 unit tests — fewer than the other modules but the surface is genuinely tiny.

**Verdict:** PASS WITH NOTES — flagging `#[doc(hidden)]` is the only practical gap; add a one-liner to the description or implement a skip when feasible.

#### 2.2.4 `fn_body_audit` (`src/graph/fn_body_audit.rs`, 809 LOC)

The largest single module in the commit. Eight pattern matchers (`unwrap`, `expect`, `panic_macros`, `unwrap_unchecked`, `transmute`, `await_in_guard_scope`, `self_recursion`, `unbounded_loop`), each as a free function over `&SyntaxNode`. Driver walks every local-crate source file, iterates every `Fn` AST node, runs the enabled subset of matchers on the body, resolves the enclosing fn (snapshot-NodeId + qualified name), and assembles a 3-line `context` snippet via `build_context`.

**Issues:**

- **(med)** **The `for ast_node in syntax_root.descendants()` outer loop catches every `Fn` AST node, including nested fns/closures-with-fn-bodies and fns inside trait impls.** Then matchers run over the inner body using `body.syntax().descendants()`. The same pattern hit inside a nested fn will be reported once attributed to the **outer** fn (via `enclosing_fn_for_body_offset`, which resolves to the *innermost* `containing_function`) — and then a **second time** when the outer loop reaches the nested fn itself. So a nested fn's `unwrap()` is double-reported. This is also true for the parent-fn-walk: the parent's `descendants()` includes the child fn's body, so the child's `unwrap()` is matched once via the parent's body and once via the child's body. **Severity: medium — produces duplicate findings whenever a fn defines a nested fn (rare but present in production code, e.g. visitor helpers). Worth filtering out, or constraining matchers to skip descendants whose enclosing fn is not the current fn.**
- **(med)** `match_unbounded_loop`'s exit detection uses a flat `descendants()` walk and returns true on any `BreakExpr`, `ReturnExpr`, or `TryExpr`. A `break` inside a *nested* `loop { ... }` (which only breaks the inner loop, not the outer) will mark the outer `loop` as "has an exit" and silence the finding. Likewise a `?` operator inside a closure (which only short-circuits the closure body, not the outer fn) is treated as an exit. **Severity: medium — produces false negatives on legitimate unbounded-loop violations.** The right fix is to stop the walk at the boundary of a nested loop / closure / nested fn.
- **(low)** `await_in_guard_scope` walks only `block.statements()` of the *nearest* `BlockExpr` ancestor. If the `let _g = mutex.lock()` was made one block out (e.g. `{ let g = m.lock(); { foo().await } }`), it'd be missed. The plan was explicit that this is a heuristic and accepts false negatives; documented. The needle list includes a bare `"Guard"` substring, which catches `OwnedGuard`, `RcuGuard`, etc., but also any user type whose name contains "Guard" (e.g. `SecurityGuard`); acceptable false-positive surface for a v1 review trigger.
- **(low)** `match_self_recursion` covers both `CallExpr` (path) and `MethodCallExpr` (method dispatch) — better than the plan, which only specified `CallExpr`. Good catch.
- **(low)** `enclosing_fn_for_body_offset` is called with `body_syntax.text_range().start()` — i.e. the position of the body's `{`. This is inside the fn token range, so `containing_function` should return the right fn. It does not handle nested fns specifically — a fn defined inside another fn's body will resolve to itself (correct). No issue.
- **(low)** `build_context` uses byte slicing on UTF-8 boundaries derived from `rfind('\n') + 1` and `find('\n')`. Since `\n` is ASCII, this is safe. But the slice itself isn't validated as a UTF-8 boundary — `&file_text[prev_line_start..next_line_end]` will panic if those bounds happen to land mid-multibyte-char. In practice the bounds always sit immediately after a `\n` or `0` or `file_text.len()`, so they're on char boundaries. OK.
- **(low)** `file_text_cache: HashMap<FileId, String>` is built per-call but only ever inserted then read once (the loop iterates each `file_id` exactly once). The cache is dead code. Cosmetic.
- **(info)** The async-walk uses `parse_guess_edition`, which is fine; the test parser hardcodes Edition2024. Acceptable.
- **(info)** Pattern names `parse_pattern_filter` rejects unknowns with `format!("unknown pattern \`{n}\`; valid: {valid:?}")` — clear, debug-formatted. The router wraps as `invalid_params`. Good.

**Tests:** 18 unit tests — covers every matcher (unwrap fires, doesn't fire; expect fires; panic_macros fires on all four; doesn't fire on println; unwrap_unchecked; unbounded_loop with/without break/return/for/while; await_in_guard_scope with let / without let / let-after-await / type-annotation case). **The nested-fn double-count and the nested-loop false-negative described above are not exercised by tests.**

**Verdict:** NEEDS WORK on the nested-fn double-count and the nested-loop break-detection; otherwise robust and well-tested.

#### 2.2.5 `recursion_check` (`src/graph/recursion_check.rs`, 336 LOC)

**What it does:** pure-snapshot read. Builds an adjacency list (`caller fn` → distinct callees) from `usages_by_consumer_function` × `usages_by_id`. For each fn `start`, runs a bounded DFS up to `max_cycle_length`, collecting any path that closes back to `start`. Canonicalizes each cycle by rotating the lowest-`NodeId` (byte-lex order) to position 0; dedup via `HashSet<Vec<NodeId>>`. With a crate filter, the DFS still runs from **every** fn (so cross-crate mutual recursion is caught), but cycles are then filtered to those touching the in-scope set.

**Issues:**

- **(med)** **Quadratic-in-the-worst-case workload.** `find_cycles_from` is invoked for every fn (N), and each invocation does a bounded DFS up to depth `max_cycle_length`. For a workspace with several thousand fns, this is N × D × avg-branching DFS — fine at depth 5 with bounded branching, but the description hints at "10ms for 2000 fns × depth 5"; in practice each fn touched costs an LMDB lookup. Acceptable for now; consider a SCC-style Tarjan/Johnson algorithm if the tool becomes a CI hot path. **Severity: low to medium — performance concern, not correctness.**
- **(low)** Adjacency-list building uses `seen.insert(usage.target)` to dedup multiple call sites of `a → b` into a single edge. This is correct for cycle detection but loses multiplicity. Not used downstream, so fine.
- **(low)** The DFS's depth check is `if path.len() > max_depth { return; }` at the top plus `if path.len() >= max_depth { continue; }` before pushing — together they cap `path.len() <= max_depth`. For `max_depth = 1` (self-loop), the start node sits at `path.len() == 1`, so the inner `for next in outgoing(current)` runs, and a self-edge (`next == start`) emits a length-1 cycle. Correct.
- **(low)** `canonicalize_cycle` rotates to the lowest-id member but does **not** consider the reverse direction. Two cycles `a → b → c → a` and `a → c → b → a` would be considered distinct, which is correct for a directed graph (different edges traversed). Good.
- **(info)** The crate filter is described in the tool description as "a cycle is included if at least one of its members lives in the requested crate" — which is the deliberately looser semantics the plan called for. Good documentation.
- **(info)** "Bounded DFS to depth N" plus per-start traversal means each cycle is independently rediscovered from each of its members; dedup via canonical form makes this O(C × L) extra work per cycle of length L. Fine for typical recursion depths.

**Tests:** 8 unit tests covering self-loop, 2-cycle, 3-cycle, terminal (no cycle), depth-bounded exclusion, branching that emits multiple cycles, and the canonicalization rotation (lowest-id first, single-element identity). Solid coverage of the algorithmic core; no test wires the full snapshot path.

**Verdict:** PASS — the algorithm is correct and tested in isolation.

### 2.3 `2e2e7fb3` — `ast_resolve` helper (119 LOC)

**What it does:** introduces `src/graph/ast_resolve.rs` with a single function `resolve_call_to_function(sema, &CallExpr) -> Option<Function>`. The implementation routes through `Semantics::resolve_expr_as_callable` and pattern-matches on `CallableKind::Function`. `channel_audit.rs` and `fn_body_audit.rs` are migrated to use it (three call sites — channel-audit's main loop, fn_body_audit::match_transmute, fn_body_audit::match_self_recursion CallExpr branch).

**What was broken:** `Semantics::resolve_path` returns `None` when the path expression contains a `GenericArgList` (turbofish), e.g. `tokio::sync::mpsc::unbounded_channel::<MyType>()` or `std::mem::transmute::<u8, i8>(x)`. So before the fix, any turbofish channel construction, every turbofish self-recursion, and the specific turbofish form `std::mem::transmute::<...>(...)` were silently invisible to the audits. Real codebases use `transmute::<A, B>(...)` form heavily.

**Issues:**

- **(low)** The fix is well-scoped. The doc comment is clear about the failure mode and the rationale (type inference resolves aliases). The function returns `None` for tuple-struct / tuple-enum-variant constructors / closures / fn pointers, which is exactly what the calling matchers need.
- **(info)** **No tests** in `ast_resolve.rs` itself (the file imports nothing test-related). A unit test exercising `resolve_call_to_function` against a hand-built `CallExpr` is hard (needs a `Semantics` over a `RootDatabase`), so the lack of unit tests is defensible. A regression test in the form of a fixture with a turbofish call would catch the bug if it returned. **Severity: info — no test, defensible reason.**
- **(info)** The fix is **complete** for the modules in this commit. `Semantics::resolve_path` is not used anywhere else in `src` (verified — only its mention is in tool-description strings). The only other callsite that walks `CallExpr`s, `src/parser/call_graph.rs::extract_call_target`, uses pure syntactic matching (`path.segments().last().name_ref()`) so it isn't affected by the turbofish issue.

**Verdict:** PASS — small, focused, correct.

## 3. Cross-commit observations

### 3.1 Was the `ast_resolve` fix complete?

Yes. The fix is applied to all three sites that used the broken `Semantics::resolve_path → PathResolution::Def(ModuleDef::Function)` pattern: `channel_audit.rs`'s main `CallExpr` loop, `fn_body_audit.rs::match_transmute`, and `fn_body_audit.rs::match_self_recursion`'s `CallExpr` branch. The `MethodCallExpr` branch of `match_self_recursion` uses `sema.resolve_method_call`, which is a different API and unaffected by turbofish in the *path* (turbofish on a method call sits on the method name, not the receiver path, and `resolve_method_call` is the right API for that).

No other module in `src/` uses `Semantics::resolve_path`. `parser/call_graph.rs` uses syntactic-only matching, so it is not vulnerable. The fix is workspace-complete.

### 3.2 Plan vs implementation fidelity

The implementation is faithful to the plan. Specific notes:

- **Tool 1 (`fn_body_audit`):** all 8 patterns shipped with the names from the plan. The plan said `unwrap_unchecked` would only match `unwrap_unchecked` / `unwrap_err_unchecked` — both are implemented. The plan suggested `self_recursion` would resolve via `PathExpr` `CallExpr`s only; the implementation **also handles `MethodCallExpr` via `resolve_method_call`**, a deliberate over-delivery. Default-pattern-set is "all 8 enabled" (matches the plan's recommendation #1).
- **Tool 2 (`derive_audit`):** matches the plan including the path-stripping rule (`serde::Serialize` → `Serialize`). The optional `item_kind` was added as a list (the plan said a single string); the tool-level glue rejects non-{Struct, Enum, Union} kinds with `invalid_params`. Reasonable extension.
- **Tool 3 (`missing_docs_audit`):** matches the plan. **The plan did not call out `#[doc(hidden)]` and neither does the implementation** — both will flag a `#[doc(hidden)] pub fn` as missing docs. The plan's `#[doc = "..."]` caveat is not addressed in the tool description.
- **Tool 4 (`channel_capacity_audit`):** matches the plan, with two extra resolvable canonical paths (`tokio::sync::mpsc::bounded::channel`, `tokio::sync::mpsc::unbounded::unbounded_channel`) handled — defensive coverage for whatever path RA returns after `pub use` resolution.
- **Tool 5 (`recursion_check`):** matches the plan. The default cycle length is 5, hard cap 12 (as planned). The "looser" crate-name filter (cycle included if any member is in-scope) is documented in both the plan and the tool description.

The 3,247-LOC overage vs the plan's ~1,300-LOC estimate is fully explained by: (a) very long `#[tool(description = "...")]` strings on each route (the descriptions are essentially mini-docs and add ~100 LOC each, ~500 LOC together), (b) larger-than-projected unit-test suites (the channel module alone has 19 tests, ~120 LOC), (c) sizeable serialization-renderer structs in `graph_tools.rs`, and (d) the TOOLS.md entries (~320 LOC of documentation).

### 3.3 Tool surface and argument validation

All five new param structs follow the existing project conventions: `#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]`, fields annotated with `#[schemars(description = ...)]`, `Option<...>` for defaults. Validations done at the tool-handler layer (`graph_tools.rs`):

- `derive_audit::required_derives` empty-check (correct).
- `derive_audit::item_kind` rejects non-{Struct,Enum,Union} (correct).
- `recursion_check::max_cycle_length` is clamped to `[1, HARD_CAP_CYCLE_LENGTH=12]` (correct).
- `fn_body_audit::patterns` validated by `parse_pattern_filter` against `ALL_PATTERNS` (correct — unknown names error out).
- The Crate/Module resolution wrapper coerces a root Module → Crate via `node.crate_id.or(node.parent_id)`; same wrapper duplicated in five places (could be deduplicated to a helper, minor refactor opportunity).

JSON-schema consistency: all five tools use `(u32, u32)` for spans, all return a `scope: { directory, crate_name? }` envelope, and `finding_count`. Consistent and predictable.

Error handling: `internal_error("<context>")` for snapshot errors; `McpError::invalid_params` for user errors. `spawn_blocking` is used for the two AST-walk tools (channel / fn_body), matching `unsafe_audit`'s pattern; the join error is mapped to `McpError::internal_error`. No panics.

### 3.4 Test coverage

19 + 4 + 13 + 18 + 8 = 62 unit tests for ~2,300 LOC of audit code (~1 test per 37 LOC). Per-module distribution is reasonable. Gaps:

- No integration test that exercises the full `loader::load` + audit pipeline (would catch issues like the turbofish bug pre-`2e2e7fb3`).
- No regression test for the turbofish bug — adding a fixture with `tokio::sync::mpsc::unbounded_channel::<u8>()` or `std::mem::transmute::<u8, i8>(0)` and asserting the audit catches it would protect against re-regression.
- `fn_body_audit`'s nested-fn double-count and nested-loop break-detection (see §2.2.4) are not exercised.

## 4. Overall verdict

**MINOR.**

The feature is well-scoped, well-documented, faithful to the plan, and lands a real bug fix in `2e2e7fb3`. The plan document is unusually clear and the implementation matches it without significant deviation. The five tools each carry a proportionate test suite, follow existing project conventions, and produce consistent JSON output.

Two non-trivial issues in `fn_body_audit` (medium severity) deserve attention:

1. **Nested-fn double-count** — a fn body containing a nested `fn` produces duplicate findings, once attributed to the outer fn (via the outer's descendants walk) and once to the inner. Real production code defines nested fns rarely but non-zero (visitor helpers, formatter callbacks). Fix: stop descending into nested `Fn` AST nodes, or attribute strictly to the *deepest* enclosing fn at the call site rather than the outer-loop fn.
2. **`unbounded_loop` over-permissive exit detection** — `break` / `return` / `?` are all matched via flat `descendants()`, so a `break` inside an inner nested `loop` (which doesn't break the outer) silences the outer-loop finding. Fix: short-circuit the walk at boundaries of nested `LoopExpr` / `ClosureExpr` / `Fn`.

Two minor improvements in `docs_audit`:

3. Add `#[doc(hidden)]` recognition (skip those items from "missing docs").
4. Optionally recognize `#[doc = "..."]` attributes as documentation.

None of these block merge. The feature is shippable as-is; the gaps above are clear v1.x follow-ups.
