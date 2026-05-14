# G5 Review — Layer 4 impl-item extraction + Layer 10 call graph

Commits reviewed (chronological):

| short | LOC | title |
|---|---:|---|
| `e51f778b` | 428 | [review] add layer 4 impl item extraction for methods, assoc consts, assoc types |
| `fcd3eadc` | 438 | [review] add layer 10 fn-scope call graph with who_calls and calls_from |
| `ffb9e22a` | 559 | [review] add call_graph and recursive_callers_count queries and tools |

## 1. Group summary

This group lands two cleanly layered additions:

- **Layer 4** (e51f778b) makes inherent-impl methods, associated consts, associated types, and trait-declaration methods first-class `Item` nodes. The new pass walks `Impl::all_in_crate(db, krate)` for inherent impls and `trait_.items(db)` for trait declarations, then plugs the resulting `ModuleDefId → NodeId` entries into `def_to_node` **before** `extract_usages` runs. That single ordering decision is the load-bearing trick: it makes the existing `Definition::usages` machinery in `extract_usages` automatically yield Usage rows for `Foo::bar`, `Trait::method`, etc. with no second pass.
- **Layer 10** (fcd3eadc) adds per-Usage `consumer_function: Option<NodeId>` and a `usages_by_consumer_function` DUP_SORT sub-DB, then exposes the two direct-query tools `who_calls` and `calls_from`.
- **Layer 10b** (ffb9e22a) builds three derived queries on top: `call_graph` (bounded DFS over outgoing edges with cycle/depth truncation), `callers_in_crate` (caller-crate filter on `who_calls`), and `recursive_callers_count` (reverse BFS that returns the *distinct caller fn* count).

The three commits compose: Layer 10 directly depends on Layer 4's expanded `def_to_node` to attribute call sites correctly (because the enclosing fn lookup `def_to_node.get(&ModuleDefId::FunctionId(id))` would otherwise miss every method body).

Schema is bumped twice (v4→v5 in e51f778b, v5→v6 in fcd3eadc). Both bumps are guarded by `graph_id_for` mixing `SCHEMA_VERSION`, so stale snapshots auto-rebuild.

## 2. Per-commit review

### `e51f778b` — Layer 4 impl item extraction (428 LOC)

**What it does.** Adds `src/graph/impls.rs::extract_impl_items` (run in `extract.rs` between bindings and usages). For every local crate it iterates `Impl::all_in_crate`, skips trait impls (`impl T for Foo`), and for each remaining inherent impl emits `Item` nodes for fns (kind `Method`), consts (`AssocConst`), and type aliases (`AssocType`). Then it walks every local `TraitId` already in `def_to_node` and emits the same three kinds for trait-declaration items (default-body fns included).

Adds `ItemKind::Method`, updates every ItemKind→label switch (`bindings.rs::node_kind_label`, `queries.rs::display_kind`, `graph_tools.rs::usage_kind_label` and the second `display_kind`). Bumps `SCHEMA_VERSION` (v4→v5) so old snapshots auto-rebuild via `graph_id_for`. Rewrites the previously-`_NOT_CAPTURED_` tests in `usages.rs` to assert that `Foo::bar` and `Trait::method` are now usage-resolvable.

**Issues.**

- **MINOR — visibility/attributes dropped.** `emit_assoc_item` always sets `visibility: None` and (in the version visible from `HEAD`) `attributes: Vec::new()`. `pub fn`, `pub(crate) fn`, `#[cfg]`, `#[deprecated]`, etc. on impl-block items are not surfaced. This propagates into `dead_pub_*` semantics: a `pub fn` inside an inherent impl whose host type is local will read as `visibility=None` and therefore can't be a "dead pub" candidate. Acceptable as a v1 scope decision, but the assoc-const/type case is harder to defend (associated consts in traits are public-API surface). Worth a comment in `model.rs` flagging the omission, or a follow-up to backfill visibility from RA's `Function::visibility`/`Const::visibility`.
- **MINOR — redundant defensive check.** `if adt.krate(db) != krate { continue; }` (impls.rs:81–86) — Rust coherence already guarantees an inherent impl's self-ty lives in the same crate as the impl. The check is harmless but the comment ("inherent impls of types declared in dep crates aren't tracked in v1") describes a different scenario (and `adt_node_for.get(&adt_id)` on the line above already handles the out-of-workspace case).
- **MINOR — silent skip on missing nav.** `try_to_nav` returning `None` (macro-only / synthetic items) drops the item without any logging. Combined with the byte-offset NodeId scheme, a future maintainer could spend time debugging "why isn't this method in the graph?" with no signal. A debug `tracing::trace!` would be a cheap improvement.
- **NIT — `_ => unreachable!()` in `kind_label` match.** Reachable in theory if a new `ItemKind` is added without updating this site — would crash at extraction. Either pattern-match exhaustively or pattern-match the 3 expected variants and `unreachable!` with a message naming the unexpected variant.
- **NIT — trait-impl-body items deliberately omitted (Layer 4c) — but this means inherent-impl items are first-class while trait-impl items are not.** The docstring at the top of `impls.rs` explains this is intentional (RA resolves `x.m()` back to the trait decl), and the deferred-work bullet in the design notes lists "Layer 4c". The note about why is good. However, **see the Layer 10 issue below: this gap propagates to `consumer_function = None` for refs from trait-impl bodies**, which IS NOT explicitly documented in the user-facing tool descriptions.

**Verdict.** PASS. Clean, well-scoped, and the ordering-before-usages trick is elegant. The omissions (visibility, attrs) are deferrable. Test coverage upgrades the previous-gap pattern1/pattern2 cases from `_NOT_CAPTURED_` proxies to direct `_captured` assertions.

---

### `fcd3eadc` — Layer 10 fn-scope call graph (438 LOC)

**What it does.** Adds `Usage::consumer_function: Option<NodeId>` (with `#[serde(default)]` so the field is forward-tolerant). In `extract_usages`, after computing `consumer_module` from the file-root scope, re-runs `sema.scope_at_offset` on the token's parent so RA's `find_container` can walk past the file root and reach a `DefWithBodyId`; calls `containing_function()` and looks up `def_to_node[ModuleDefId::FunctionId(id)]`. The result is stored on each Usage.

Adds a new `usages_by_consumer_function: NodeId → UsageId` DUP_SORT sub-DB, populated alongside `usages_by_consumer` in `snapshot.rs`. Exposes `OpenedSnapshot::who_calls` and `OpenedSnapshot::calls_from`, both returning `Vec<EnrichedCallSite>` (a new shape with `caller_qualified_name + callee_qualified_name + file + start + end + category`). MCP tools `who_calls` and `calls_from` and matching param structs are wired in `graph_tools.rs` / `search_tool.rs` / `search_tool_router.rs`. Enriches the `who_uses` JSON response to include a `consumer_function` field.

Bumps `SCHEMA_VERSION` v5→v6 — required because bincode reads of v5 records would EOF on the new `consumer_function` field even with `#[serde(default)]`.

**Issues.**

- **MEDIUM — undocumented coverage gap: trait-impl method bodies.** `extract_impl_items` deliberately skips trait impls, so a trait-impl `fn m`'s `FunctionId` never enters `def_to_node`. When the Layer 10 attribution does `def_to_node.get(&ModuleDefId::FunctionId(id))` for a reference inside `impl T for Foo { fn m { other(); } }`, it returns `None` and the Usage row records `consumer_function = None`. Net effect:
  - `who_calls(other)` does NOT include this call site.
  - `calls_from(...)` cannot be rooted at the trait-impl fn.
  - `who_uses(other)` DOES include the call site (because consumer_module is still set), so users have a fallback — but the discrepancy is silent.

  The tool docstrings list the documented `None` cases (const initializers, trait bounds, enum discriminants) and tell users to fall back to `who_uses` — but trait-impl method bodies aren't on that list, even though they're a much more common path. **Recommendation:** add a line to the `who_calls` / `calls_from` tool descriptions, and to the corresponding query docstrings, naming trait-impl bodies as another `consumer_function = None` source (or, better, plan Layer 4c to close the gap).

- **MINOR — `EnrichedCallSite::category` is a `String`** ("Read"/"Write"/"Test"/"Other") rather than a reused enum / `&'static str`. The internal storage already has `UsageCategory` (a bitflags-like type) and the file already has `usage_category_label` returning `&'static str`. Allocating a `String` per row is wasteful at high fan-out and breaks pattern matching downstream. Recommendation: use `&'static str` (the JSON serializer doesn't care) or expose the enum directly.
- **MINOR — `who_calls` reports per-call-site rows, not per-caller.** A caller fn that invokes the target 5 times produces 5 rows with identical `caller_qualified_name` and different ranges. That's a defensible design (`call_sites: Vec<...>`) but the tool description says "every fn-body reference to a target fn (caller-attributed)" — slightly ambiguous about whether duplicates are de-duped. The included `recursive_callers_count` test expressly distinguishes "counts fns, not call sites", so the per-call-site shape here is consistent — but worth a one-line clarification in the `who_calls` description.
- **MINOR — `Between` token branch prefers the right token.** In `usages.rs:127`, `TokenAtOffset::Between(a, b) => b.parent().or_else(|| a.parent())` picks the right neighbour first. For most refs the range starts on a non-whitespace, non-trivia token so this is rare; the choice doesn't appear to affect correctness because both tokens belong to the same enclosing fn in practice. NIT-level, but a comment explaining the preference would help.
- **NIT — extra `scope_at_offset` per reference.** Layer 10's algorithm does TWO `sema.scope_at_offset` calls per ref (one on `syntax`, one on the token's parent). The hyper-impl note claims ~5–10% overhead. Acceptable, but the first scope's `containing_function()` is always `None`-at-file-root by construction — the dual-scope structure is intentional, but a future refactor could probably collapse to a single call seeded directly with the deeper node.

**Verdict.** PASS with the trait-impl-body coverage note. The mechanical work (schema, sub-DB, snapshot writer, enriched JSON) is consistent and the new tests (`pattern6_function_attribution_works`, `pattern7_closure_attributes_to_parent_fn`, `pattern8_const_initializer_has_no_caller_fn`, `calls_from_returns_callees`) cover the right cases.

---

### `ffb9e22a` — `call_graph` + `recursive_callers_count` + `callers_in_crate` (559 LOC)

**What it does.** Builds three derived queries on the Layer 10 index:

- **`call_graph(root, depth)`** — bounded recursive descent over `usages_for_consumer_function`. A global `visited: HashSet<NodeId>` prevents re-expanding the same fn anywhere in the tree (cycles AND DAG fan-in collapse). `truncated_at_cycle` marks the second-visit prune; `truncated_at_depth` marks "depth ran out and there were outgoing edges". Tool: default depth 3, max 8 (silently clamped via `min(MAX_DEPTH)`).
- **`callers_in_crate(target, crate_qualified)`** — `who_calls`-equivalent that walks `usages_for_target`, drops rows where `consumer_function = None`, and filters by the caller's containing-crate qualified name. Note: filters by *caller's* crate, not target's — docstring and tool description both spell this out.
- **`recursive_callers_count(target, depth)`** — reverse BFS counting distinct caller fns. Returns `{direct_callers, transitive_callers, depth_reached, truncated_at_depth}`. Tool: default depth 3, max 8.

Adds `CallGraphNode`, `RecursiveCallersCount` data shapes (both `PartialEq/Eq/Serialize/Deserialize`) and three param structs + three router endpoints. Updates the server instructions string to list the new tools.

**Issues.**

- **MEDIUM — `recursive_callers_count` BFS opens a fresh `rtxn` per node per hop.** In the hop-expansion loop (`while hop < depth`) and the truncation peek, there's `let rtxn = self.env.read_txn()?;` inside `for fn_id in frontier`. LMDB read txns are cheap but not free, and a deep BFS on a hot fn can hit hundreds of nodes. Hoisting the rtxn outside the inner loop (one txn per hop, or one per call) is straightforward — there are no writes in between. Functionally correct, just inefficient.
- **MINOR — silent depth clamp.** `let depth = params.depth.unwrap_or(DEFAULT_DEPTH).min(MAX_DEPTH);` in both `call_graph` and `recursive_callers_count`. The response payload includes the (clamped) `depth`, so a caller who passed 100 and got 8 can detect it by comparing — but only if they remember to compare. No log line, no `invalid_params` error. Acceptable for an MCP surface where the tool description names the cap, but a `tracing::debug!("call_graph depth clamped from {} to {}", requested, depth)` would help.
- **MINOR — `call_graph_rec` opens `rtxn` then `rtxn2` for separate lookups.** The node-info read and the callee-iteration read are in two distinct read txns (with a `drop(rtxn)` between). One rtxn for the whole frame is simpler and the same read-snapshot. Same comment as above: functionally correct, just unnecessarily chatty against LMDB.
- **MINOR — empty `krate` parameter not rejected.** `callers_in_crate(target, "")` will walk every Usage row, find every caller, look up its crate's `qualified_name`, and compare it to `""` — always false, returns empty. Loud zero. Should probably reject empty `krate` with `invalid_params`.
- **MINOR — `EnrichedCallSite::category` is `String` (same observation as fcd3eadc).** Re-used here in `callers_in_crate`; same allocation cost.
- **MINOR — `call_graph_rec` recursion is unbounded by the depth cap at the tool layer but is bounded by `depth` at the query layer.** That's fine — the cap (8) is enforced in `graph_tools::call_graph` before calling into `OpenedSnapshot::call_graph`. But anyone calling `OpenedSnapshot::call_graph(root_fn, u32::MAX)` directly would get a stack-overflow risk on a pathological program. Since the recursive descent is depth-limited and `visited` short-circuits cycles, the actual recursion depth is bounded by `min(depth, |visited_set|)`. The visited set caps at the number of distinct local fns reachable, which on any real workspace is bounded. So safe in practice. NIT-level.
- **NIT — `truncated_at_depth` in `recursive_callers_count` does a O(frontier × refs) peek pass at the end.** This is the only way to distinguish "BFS naturally terminated" from "BFS hit the depth limit with more to do", but it's a non-trivial extra scan. Consider materializing the next-hop frontier first, checking its size, then dropping it if hop==depth — but only worth it if profiling shows it as hot.
- **NIT — `depth=0` path returns `direct_callers: 0`, `transitive_callers: 0`.** The test asserts this. The docstring says "depth=0 returns zeros" but it's worth a one-line UX note in the tool description that `depth=0` is essentially "yes/no this target exists" (it doesn't even count direct callers).

**Verdict.** PASS with the LMDB-txn churn caveat. Cycle detection and depth truncation are correct, tests cover the depth-0, depth-monotonic, and bogus-crate-filter cases. The MCP tool wiring (param schema, default/cap constants, router descriptions, instructions string) is consistent across the three new tools.

## 3. Cross-commit observations

- **Schema bump discipline is good.** v4→v5→v6 in two consecutive commits each with a 7–10 line comment in `storage.rs` explaining the change, and `graph_id_for` mixes `SCHEMA_VERSION` so old snapshots fall through to a rebuild instead of mis-decoding.
- **`ItemKind::Method` is a shared variant for inherent + trait-decl fns** — distinction goes through `parent_id`. The model docstring (model.rs:36–41 in the diff) explicitly calls this out. No separate `ItemKind::TraitMethod` / `InherentMethod` split — that's a deliberate v1 simplification. Reasonable.
- **Layer 4 is a hard prerequisite for Layer 10 attribution to work.** `extract_impl_items` runs in `extract.rs` BEFORE `extract_usages`, which is what makes `consumer_function` resolve to a Method NodeId rather than `None`. The two commits MUST land together or be reverted together; in this group they do. There is no test that asserts this ordering explicitly — `pattern6_function_attribution_works` would still pass if `extract_impl_items` ran *after* `extract_usages` for module-level free fns. A small regression-guard test that asserts `consumer_function.is_some()` on a Usage row attributed to a Method node (i.e. one whose parent_id is a struct Item) would close that.
- **Trait-impl bodies are the missing-third-leg of the Layer 4 design.** This is documented as "Layer 4c, deferred" in the design notes the diff edits, but it isn't surfaced in user-visible tool descriptions. As noted under fcd3eadc, the practical impact is that `who_calls` / `calls_from` silently lose call sites originating from `impl T for Foo { fn m { ... } }`, and `who_uses` is the only fallback. This should at minimum be mentioned in the `who_calls` and `calls_from` tool docstrings; ideally Layer 4c lands as a follow-up.
- **No conflicts between the two new node/edge kinds.** Layer 4's new `Method/AssocConst/AssocType` Items use `kind_label` strings that don't collide with bindings.rs's existing labels, and Layer 10's new `usages_by_consumer_function` sub-DB is a fresh LMDB database name. The two changes coexist cleanly.
- **Tool depth-cap constants are duplicated.** `DEFAULT_DEPTH = 3` and `MAX_DEPTH = 8` are defined `const` inside each of `call_graph` and `recursive_callers_count` in `graph_tools.rs`. Trivially fine, but if a third bounded-traversal tool lands it'd be worth a shared constants module.
- **Documentation file `.docs/hyper-impl.md` was edited by both Layer-4 and Layer-10 commits** but no longer exists in the current tree (later commits in the group removed/renamed it). Not a G5 issue but worth flagging if the reader expects the file to be present.

## 4. Overall verdict

**PASS — MINOR fixups recommended.**

The two layers compose correctly, schema migrations are disciplined, and the new MCP tools are wired consistently. Cycle handling in `call_graph` and BFS termination in `recursive_callers_count` are correct. Tests cover the right cases (depth-0, depth-monotonic, closure-attribution, const-init-None, bogus-crate-filter, call_graph cycle-truncation via the visited-set test).

Recommended follow-ups in rough priority order:

1. **Document the trait-impl-body coverage gap** in the `who_calls`/`calls_from`/`callers_in_crate` tool descriptions (one extra line each). Refs inside `impl T for Foo { fn m { ... } }` give `consumer_function = None` and are invisible to the Layer 10 queries; users currently won't know to fall back to `who_uses`.
2. **Hoist the `read_txn()` out of the inner BFS loops** in `recursive_callers_count` (and consolidate the two-rtxn pattern in `call_graph_rec`) — one txn per query is sufficient and cheaper.
3. **Backfill `visibility` (and ideally attributes) on Layer 4 Item nodes.** `pub`/`pub(crate)` on assoc consts and methods is real public-API surface and currently disappears, which silently weakens `dead_pub_*` recall for impl-block items.
4. **Reject empty `krate` in `callers_in_crate`** with `invalid_params`.
5. **Replace `EnrichedCallSite::category: String` with `&'static str`** (the existing `usage_category_label` already returns one).
6. **Plan Layer 4c** (trait-impl method bodies as first-class items) to close the call-graph attribution gap at the source rather than via docs.

None of these block the group.
