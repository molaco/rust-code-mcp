# G7 Review: Function signatures + HIR display

Branch context: `rust-code-mcp-final` Phase 5 work.
Commits reviewed: `be03b17c` (signature extraction + filter), `be099986` (hir_trim helper + pagination/summary + items_with_attribute anchoring + manifest soft-fail + NodeId hex rendering).

## 1. Group summary

G7 builds the per-function signature subsystem in two strokes:

1. **`be03b17c`** introduces `FunctionSignature` (params, return type, generics, async, self-kind), wires `extract_signatures` into the extraction pipeline, adds a new `signatures_by_target` LMDB sub-DB (bumping `SCHEMA_VERSION` from 8 to 9), and exposes two MCP tools (`function_signature`, `functions_with_filter`).
2. **`be099986`** adds the `hir_trim` string-rewriting helper used by `signatures.rs` and `statics.rs` to scrub default std-library type parameters (`Global`, `RandomState`, `BuildHasherDefault<…>`, `LazyLock<X, fn() -> X>`) from `HirDisplay` output. The same commit also lands a sizable bundle of unrelated wrapper work: pagination/summary for `functions_with_filter`, an anchored matcher for `items_with_attribute`, NodeId hex rendering for `mut_static_audit` / `unsafe_audit` / `crate_dependency_metric`, plus a soft-fail manifest reader.

The two commits together form a working signature surface (extraction → persistence → query → MCP tool). The hir_trim helper is the load-bearing piece for any downstream consumer that pattern-matches on rendered types, and it deserves the bulk of the scrutiny below.

## 2. Per-commit review

### `be03b17c` — add function signatures extraction and signature-filter query tools (Phase 5)

LOC: 746 (10 files, +708/-19).
Files: `src/graph/extract.rs`, `src/graph/mod.rs`, `src/graph/model.rs`, `src/graph/queries.rs`, `src/graph/signatures.rs` (new), `src/graph/snapshot.rs`, `src/graph/storage.rs`, `src/tools/graph_tools.rs`, `src/tools/search_tool.rs`, `src/tools/search_tool_router.rs`.

**What it does**

- New `FunctionSignature { is_async, self_param: Option<SelfKind>, params: Vec<Param>, return_type, generics: Vec<GenericBound> }` model. `Param { name, ty, by_ref, mutability }`; `GenericBound { name, bounds }`. (`src/graph/model.rs:84-139`)
- New extraction pass `extract_signatures` iterates `def_to_node` for `ModuleDefId::FunctionId`, derives a per-crate `DisplayTarget` (cached), and reads `func.params_without_self`, `func.self_param`, `func.ret_type`, `func.is_async`, `func.is_async`, and `GenericDef::from(func).type_or_const_params` (filtering out `is_implicit`). (`src/graph/signatures.rs:38-138`)
- LMDB sub-DB `signatures_by_target` (Bytes → `SerdeBincode<FunctionSignature>`), not DUP_SORT. (`src/graph/storage.rs:265-280`)
- `SCHEMA_VERSION` bumped 8 → 9; existing snapshots will rebuild because the hash includes `SCHEMA_VERSION`. (`src/graph/storage.rs:85`)
- Two new queries: `function_signature(target)` (single-key fetch) and `functions_with_filter(crate_id, FunctionFilter)` (full-iter + per-key node fetch + filter predicate). Sorted by qualified name. (`src/graph/queries.rs:1032-1077`, predicate at `2282-2305`)
- MCP wrappers `function_signature` / `functions_with_filter` in `graph_tools.rs:512-616`; `self_kind` parameter parses `"none"|"owned"|"ref"|"ref_mut"`.
- Five unit tests in `signatures.rs::tests` and three filter smoke tests.

**Issues**

| Severity | Finding |
|---|---|
| Medium | **No capture of `const fn`, `unsafe fn`, or `extern "abi"` fn modifiers.** `FunctionSignature` carries `is_async` only. A const fn and a non-const fn read identically. `Function::is_const(db)`, `Function::is_unsafe(db)` exist on RA and should be added before this becomes downstream-visible (else later additions force a SCHEMA_VERSION bump and a snapshot rebuild for every consumer). |
| Medium | **No explicit lifetime parameters / where-clauses.** `GenericBound` collects type-param trait bounds only. Functions with lifetimes (`fn foo<'a>(...)`) or where-clauses (`where T: Iterator<Item = X>`) lose those signals; the only place they survive is inside the stringified param `ty` / `return_type`, which is fine for substring filters but not for structured tooling. The commit message says nothing about this gap, and the `GenericBound` doc-comment only flags the `trait_bounds` partial-view caveat (RA's FIXME), not the absence of lifetime/where coverage. |
| Low | **`Param.idx` is captured then discarded.** `signatures.rs:67` does `let idx = p.index(); let _ = idx;` — pure noise; either drop it or store it. The trailing comment ("preserved by iteration order") is correct so the index is genuinely unused. |
| Low | **`functions_with_filter` is O(#fns × log-node-fetch) per call.** Iterates `signatures_by_target`, performs one `nodes_by_id.get` per entry to scope by crate, then sorts. Acceptable in practice (signatures are bounded by local fns), but there is no early-out when `crate_id` is invalid. The wrapper validates `node.kind` is a Crate / Module first, so callers cannot easily blow it up, but a workspace with many crates pays the scan cost per crate-call. |
| Low | **`functions_with_filter` substring matches are case-sensitive against HirDisplay strings**, which is fine but undocumented in `model.rs`. The router-tool description does call this out. |
| Low | **`SelfKindFilter::None` doubles as "no self".** Naming is loaded — `Option<SelfKindFilter>::None` (no constraint) vs `Some(SelfKindFilter::None)` (must be no self). Tests cover the latter case; doc comment on `SelfKindFilter` at `queries.rs:269-273` does call this out. Acceptable but worth a callout. |
| Low | **Closure signatures are not extracted.** The pass only walks `ModuleDefId::FunctionId`. Closures don't have FunctionIds in RA's def map, so the omission is principled, but `FunctionSignature`'s doc should say so explicitly (currently it implies "every local fn"). |
| Low | **Inherent vs trait methods are conflated.** Both end up in `signatures_by_target` with no distinguishing flag. The Node already carries the parenting structure, so callers can recover it, but a `kind: FunctionKind` enum (Free / Inherent / TraitDecl / TraitImpl) on `FunctionSignature` would be a small free win and avoid post-hoc joins. |
| Low | **`tracing::trace!` on the skip-path.** `signatures.rs:43-49` traces when `build_signature` returns `None`, but `build_signature` always returns `Some` — so this is dead code. Either propagate a real failure mode from `build_signature` or drop the `Option`. |

**Tests**

`signature_loader_load`, `signature_opened_snapshot_usages_of`, `signature_workspace_stats_is_async`, `signature_node_id_from_components` plus three `functions_with_filter` smokes. These exercise the **field-level shape** (`params.len()`, `self_param`, `is_async`, `return_type.contains(...)`), not the full stringified output. That choice insulates them from the later hir_trim changes — which turns out to be deliberate (see cross-commit notes).

**Verdict**: PASS with documentation/schema follow-ups. The model is sound and the LMDB schema bump is correctly hashed into `graph_id_for`. The fact that `is_const` / `is_unsafe` / lifetimes / where-clauses are not captured deserves either a follow-up commit or an explicit "Phase 5 limitations" callout in `model.rs` — otherwise the very next consumer will hit it and need another `SCHEMA_VERSION` bump.

### `be099986` — add hir_trim helper to strip noisy std default type params from HirDisplay output

LOC: 1063 (10 files, +965/-47).
Files: `src/graph/hir_trim.rs` (new), `src/graph/mod.rs`, `src/graph/queries.rs`, `src/graph/signatures.rs`, `src/graph/snapshot.rs`, `src/graph/statics.rs`, `src/graph/storage.rs`, `src/tools/graph_tools.rs`, `src/tools/search_tool.rs`, `src/tools/search_tool_router.rs`.

**What it does**

- New `src/graph/hir_trim.rs::trim_hir_display(&str) -> String`. Four passes:
  1. `while find(", Global>") { replace }` — repeated, so `Vec<Vec<T, Global>, Global>` collapses to `Vec<Vec<T>>`.
  2. `while find(", RandomState>") { replace }` — runs after pass 1 so `HashMap<K, V, RandomState, Global>` → `HashMap<K, V, RandomState>` → `HashMap<K, V>`.
  3. `strip_build_hasher_default` — small bracket-depth walker that matches `, BuildHasherDefault<…>>` (inner may nest, e.g. `BuildHasherDefault<FxHasher>`).
  4. `strip_lazy_lock_init_fn` — bracket-depth walker that matches `LazyLock<X, fn() -> X>` and only strips when the two `X`s are textually equal.
  Defensive `tracing::trace!` if the result has unbalanced `<`/`>` counts.
- Called from `signatures.rs:96/108` (param & return type) and `statics.rs:47` (static type_string).
- *Also in this commit (not really hir_trim-related):*
  - `items_with_attribute` changes from `contains(pattern)` to anchored prefix-or-doc-body matching, with a new `match_location: "attr"|"doc"` field and empty-pattern-returns-nothing. Three tests added/updated.
  - `functions_with_filter` gains `limit` (default 50) / `offset` / `summary` params, `total_match_count` in response, three new wrapper tests.
  - `crate_dependency_metric` gains `top_n` and `sort_by` (descending; whitelist of 5 keys; unknown errors with `invalid_params`).
  - `mut_static_audit`, `unsafe_audit`, `crate_dependency_metric` switch to hex-string rendering for `NodeId`.
  - New `read_manifest_compatible` returns `Ok(None)` on schema mismatch so stale snapshots produce "no snapshot — call build_hypergraph first" instead of an opaque error.
  - `enum_variants` sort changes from "by qualified name" to "by (file, span)" (declaration order).

**hir_trim review — this is the heuristic the task brief flagged**

The trimmer is a string-rewriting heuristic over `HirDisplay` output, which the trimmer module's own doc acknowledges as risk-bearing. Specific findings:

| Severity | Finding |
|---|---|
| Medium | **Bare-name match for `Global` / `RandomState` is unconditional.** Steps 1 and 2 use `String::find(", Global>")` and `String::find(", RandomState>")` — no awareness of the path qualifier RA might emit. The module doc says `HirDisplay` always emits bare names; that is true for std types, but a user-defined `mod foo { struct Global; }` rendered as `Foo<Bar, Global>` (in the unusual case where HirDisplay's `DisplayTarget` doesn't disambiguate) would silently mangle. The accepted-risk note in the module doc is honest; the `tracing::trace!` unbalanced-bracket check catches only the structurally broken case, not the "user type named `Global`" case (the result is still balanced, just wrong). Tests do not cover this scenario at all. |
| Medium | **No test for `, Global>` at the END of the input (no nesting).** Tests cover `Vec<u32, Global>` and `Vec<Vec<u32, Global>, Global>`. Not tested: `(Vec<u32, Global>, u32)` (tuple), `fn(u32) -> Vec<u32, Global>` (fn type), `dyn Iterator<Item = Vec<u32, Global>>` (trait object). All should work given the bare-substring approach, but none are exercised. |
| Medium | **`, RandomState>` does NOT handle `HashSet<T, RandomState, Global>`.** `HashSet` after pass 1 becomes `HashSet<T, RandomState>`; pass 2 strips to `HashSet<T>`. That works, but the comment at `hir_trim.rs:36` mentions only `HashMap`. Pass 2 is not gated by `HashMap`, which is the right call but the docs should mention HashSet (and any other `, RandomState>` carrier). |
| Medium | **`strip_build_hasher_default` and `strip_lazy_lock_init_fn` are bracket walkers, but their `->` handling is brittle.** Both skip `->` to avoid mis-counting the `>` of a fn-pointer return arrow (`fn() -> T`). That helps with `LazyLock<X, fn() -> X>` and `, BuildHasherDefault<fn() -> X>>`. But the walker does NOT handle:<br>• `-` not followed by `>` (e.g. `[u8; -1]` — unrealistic for HirDisplay but the walker still byte-tests `b'-'` regardless of context).<br>• Strings or character literals containing `<`/`>` inside generic arg const-expressions. Vanishingly unlikely in HirDisplay output but a true edge case.<br>• A literal `>` inside a string in a `const N: &str` generic arg. Same — extremely unlikely in HirDisplay output. None of these are exercised; they may never matter in practice. |
| Medium | **Tool description claims `OnceLock` init-fn pointer is stripped — implementation only strips `LazyLock`.** `search_tool_router.rs:439, 447, 463` all advertise "`LazyLock`/`OnceLock` init-fn pointer parameters are stripped". `strip_lazy_lock_init_fn` only matches the needle `"LazyLock<"`. `OnceLock<T>` in std has no init-fn type param (`get_or_init` takes the closure at the call site), so the description is misleading. Either add a `strip_once_lock_init_fn` (unnecessary — `OnceLock<T>` is the canonical render) or update the tool description to drop the `OnceLock` mention. |
| Low | **No test for "no match — passthrough" on the BuildHasherDefault walker.** If the input ends after the inner `>` without the expected outer `>` (the abort path at `hir_trim.rs:93-103`), the code emits `"&s[start..idx]"` verbatim and continues. Worth a regression test, since the abort path also leaks the trailing `BuildHasherDefault<…` text into the output if structure is malformed. |
| Low | **`strip_lazy_lock_init_fn` only matches the FIRST top-level comma.** `b',' if depth == 1 && top_comma.is_none()` — so a hypothetical `LazyLock<X, Y, Z>` (3 args) would mismatch the `rhs == "fn() -> {lhs}"` check and leave the whole thing alone. That's the documented "leave it" path, so behavior is safe. Worth a passthrough test. |
| Low | **The unbalanced-bracket check is `tracing::trace!`, not `tracing::warn!`.** Trace level is filtered out by default; if the trimmer ever does miscount, the user gets a silently wrong type string with no log evidence. Promote to `warn!` (or at least `debug!`) for the duration of the "accepted risk" period. |
| Low | **Idempotence is asserted on one input.** The `trim_is_idempotent` test exercises one nested case. Worth running `trim(trim(s)) == trim(s)` over the unit-test inputs as a parameterized check. |
| Low | **The `, ` (comma-space) substring is brittle to formatting drift.** If RA's `HirDisplay` ever switches to `","` (no space) — unlikely but possible — every substring needle here breaks silently. There's no upstream-format pin / version assertion. |

**Other changes in this commit (briefly)**

- `items_with_attribute` anchored matcher: well-motivated (the legacy substring matcher false-matched on `#[tool(description = "...#[must_use]...")]`). Empty-pattern-returns-nothing is the right safer-default. Three tests added including a negative test. ALSO a behavior change for any consumer that was relying on substring semantics — the docstring is updated but a CHANGELOG note would help. Acceptable.
- `functions_with_filter` pagination/summary: clean default-limit-50 + offset + summary mode with `skip_serializing_if`. Tests cover all three knobs. One concern: the wrapper's pagination tests assert via `total_match_count > 0`, but the inline tests rely on >50 async fns existing in the workspace to "exercise the cap". The relaxed assertion (`match_count <= 50`) is correct regardless — pass.
- `crate_dependency_metric` `sort_by` / `top_n`: whitelist sort keys (5 of them) with `invalid_params` for unknowns. Sort is by NaN-equal-treated-as-Equal — fine for finite metrics. `top_n` applies after sort. Good.
- NodeId hex rendering for three audits: makes JSON readable. Done in three places via local `Rendered` structs — minor duplication, but each audit's payload differs enough that the duplication is reasonable. Worth a shared helper at some point.
- `read_manifest_compatible`: clean soft-fail for schema mismatch. Real parse errors still propagate. Caller `open_current` correctly returns `Ok(None)` so the wrapper layer reports "run build_hypergraph first". Good.
- `enum_variants` declaration-order sort: silently changes the order returned to MCP consumers. Acceptable but worth flagging in the tool's description (currently still says nothing about variant order in the router's enum tool description; I didn't see a description update in this diff for that tool — worth checking the router file separately).

**Verdict**: NEEDS WORK on the tool-description mismatch (`OnceLock` claim), the `tracing::trace!` → `tracing::warn!` promotion, and a couple of additional `hir_trim` tests. The core string-rewriting logic is sound for the documented patterns; the risk envelope is correctly documented in the module's own doc. The bundled non-hir_trim work is well-tested and correct but does dilute the commit scope.

## 3. Cross-commit observations

**Output stability**. The two commits land in order: `be03b17c` introduces signatures, then `be099986` post-processes the rendered strings via `trim_hir_display`. The first commit's tests use `.contains(...)` assertions for "Path" / "Vec" / "Result" — exactly the kind of looseness that survives the trim transform. There are no full-equality snapshot assertions on rendered type strings in `signatures.rs::tests`, so the second commit doesn't have to update them. Good defensive choice on the first commit's part; possibly accidental, but the result is that hir_trim could be inserted without rebaselining anything.

**Schema interaction with trimming**. `SCHEMA_VERSION` is 9 in both commits. `hir_trim` doesn't bump it — but the **persisted** signature strings are now trimmed-form, not raw `HirDisplay`. That means a snapshot built on `be03b17c` and read on `be099986` (or vice versa) would see different `Param.ty` / `FunctionSignature.return_type` strings without any schema-version signal. In practice the snapshot is rebuilt on every workspace change (the snapshot hash bakes the `SCHEMA_VERSION` into `graph_id_for`), and `read_manifest_compatible` now soft-fails any mismatch — but the same `SCHEMA_VERSION=9` covering both pre-trim and post-trim signature strings is a category mismatch. If someone bisects across these two commits, they could see "stale" signature strings without any visible signal. Worth bumping to v10 on the hir_trim commit (cheap — just forces a rebuild), or at least adding a runtime check at open time. Acceptable as-is because the commits flow linearly and no released artifact exists, but flag it.

**Tests on signatures.rs are crate-relative**. `sig_of("file_search_mcp::graph::loader::load")` etc. use the crate's own qualified names. Adding `is_const` / `is_unsafe` / lifetimes later won't break these specific tests because they only assert structural fields. Good — minimal lock-in.

**Description ↔ code drift risk**. The tool descriptions in `search_tool_router.rs` were updated in `be099986` to mention `LazyLock`/`OnceLock` trim. The `OnceLock` claim is false (see issue above). Tool descriptions are user-visible (LLM-facing) and not under any compile-time validation — easy to drift.

**Unrelated commit work in `be099986`**. The hir_trim commit also lands `items_with_attribute` anchoring, `functions_with_filter` pagination, `crate_dependency_metric` sort/top_n, NodeId hex rendering, and `read_manifest_compatible`. These are all sensible changes but they belong in 4-5 separate commits. Reviewing them under "[review] add hir_trim helper" required tracing through ~700 LOC of unrelated wrapper changes. Future commits should narrow scope.

## 4. Overall verdict

**MINOR** — the signature extraction is well-shaped for what it captures, the LMDB persistence + schema bump is correct, and the hir_trim heuristic correctly documents its own risk envelope and is tested for the common patterns. The blockers are minor and fixable in a follow-up:

- Fix the `OnceLock` claim in three tool descriptions (`src/tools/search_tool_router.rs:439, 447, 463`).
- Promote the unbalanced-bracket trace to `warn!` so silent miscounts get logged.
- Document the absence of `is_const` / `is_unsafe` / lifetimes / where-clauses on `FunctionSignature`, or add them (preferred — adding them later forces a schema bump and snapshot rebuild for every existing consumer).
- Consider bumping `SCHEMA_VERSION` to 10 on the hir_trim commit because the stored signature payloads change format, even though field shape doesn't.
- Add 3-4 more hir_trim tests: tuple/fn-pointer carriers of `, Global>`, BuildHasherDefault malformed-input passthrough, `LazyLock<X, Y, Z>` passthrough, idempotence across all unit-test inputs.
- Split future commits like `be099986` — five logically unrelated bundles in one is hard to review.

No bugs found that would corrupt extraction or produce wrong filter results on realistic input. The system is shippable.

## Key file paths

- `/home/molaco/Documents/rust-code-mcp-final/src/graph/signatures.rs` — extraction pass (new in be03b17c, trim call added in be099986).
- `/home/molaco/Documents/rust-code-mcp-final/src/graph/hir_trim.rs` — heuristic trimmer (new in be099986).
- `/home/molaco/Documents/rust-code-mcp-final/src/graph/model.rs` — `FunctionSignature`, `Param`, `GenericBound`, `SelfKind`.
- `/home/molaco/Documents/rust-code-mcp-final/src/graph/queries.rs` — `function_signature`, `functions_with_filter`, `filter_matches`, `match_attribute`.
- `/home/molaco/Documents/rust-code-mcp-final/src/graph/storage.rs` — `SCHEMA_VERSION=9`, `signatures_by_target` LMDB sub-DB, `read_manifest_compatible`.
- `/home/molaco/Documents/rust-code-mcp-final/src/graph/snapshot.rs` — `write_model` signature persistence; `open_current` soft-fail path.
- `/home/molaco/Documents/rust-code-mcp-final/src/graph/statics.rs` — trim call site for `mut_static_audit` type strings.
- `/home/molaco/Documents/rust-code-mcp-final/src/tools/graph_tools.rs` — MCP wrappers, pagination/summary handling.
- `/home/molaco/Documents/rust-code-mcp-final/src/tools/search_tool.rs` — `FunctionSignatureParams`, `FunctionsWithFilterParams`, `CrateDependencyMetricParams`.
- `/home/molaco/Documents/rust-code-mcp-final/src/tools/search_tool_router.rs` — tool descriptions (incl. the `OnceLock` misclaim).
