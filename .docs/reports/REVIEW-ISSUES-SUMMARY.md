# Review Issues Summary

Consolidated findings from the 12-group review of all `[review]` commits.

- **Total issues:** 44
- **Blockers:** 0
- **Medium:** 10
- **Minor:** 34

Per-group reports live alongside this file:
`G1-hypergraph-foundation-report.md` … `G11-ast-body-audits-report.md`.

---

## MEDIUM (worth fixing before next release)

| # | Group | Issue |
|---|---|---|
| 1 | G9 | `clear_cache` does not wipe the new `embeddings_by_target` LMDB sub-DB; only `build_hypergraph --force_rebuild` (new `graph_id`) invalidates it. Fixed in out-of-scope commit `f8a7378`. |
| 2 | G9 | `semantic_overlaps` holds a single LMDB write transaction across every `embed_batch_async` await, serializing concurrent callers. |
| 3 | G8 | `unsafe_audit` only detects `unsafe { ... }` blocks; misses `unsafe fn`, `unsafe impl`, `unsafe trait`. Coverage gap not documented. |
| 4 | G8 | `mut_static_audit`'s `MUT_STATIC_PATTERNS` omits `Mutex`/`RwLock`/`Atomic*`/`RefCell` (covers only `static mut`/`LazyLock`/`OnceLock`/`OnceCell`). |
| 5 | G1 | Crate nodes and their root Module nodes share `qualified_name`; `lookup_by_qualified_name` returns whichever the hash-ordered scan finds first. Commit 3 patches only one resolver call site — `who_imports("crate_name")` remains ambiguous. |
| 6 | G1 | ADTs emit two near-duplicate bindings (Type + Value namespace) with no namespace dedup in the read path. |
| 7 | G2 | `90508cd4` adds a post-hoc bindings dedup that drops the Value-namespace half of unit/tuple-struct bindings — a real semantic change in downstream `namespace` reporting, not mentioned in commit message, no test pin. |
| 8 | G5 | Trait-impl method bodies (`impl T for Foo { fn m { ... } }`) silently produce `consumer_function = None`; calls from those bodies don't appear in `who_calls`/`calls_from`. Not flagged in tool descriptions. |
| 9 | G11 | `fn_body_audit` nested-fn double-count: outer `descendants()` walk visits inner-fn bodies again. |
| 10 | G11 | `fn_body_audit` `unbounded_loop` falsely treats an inner-loop `break` as an outer-loop exit. |

---

## MINOR (cleanups)

| # | Group | Issue |
|---|---|---|
| 11 | G2 | `e68b2b1c` silently flips `prefill_caches: true→false` (restored in `90508cd4`); bisect-hostile. |
| 12 | G2 | `build_hypergraph` MCP tool description still claims `no_deps=true` despite loader using `no_deps: false`. |
| 13 | G1 | `BindingId` composition omits `BindingKind` (could collide pathologically). |
| 14 | G1 | `enrich_bindings` silently swallows `read_txn` errors. |
| 15 | G1 | Debug examples hardcode personal-machine paths. |
| 16 | G1 | O(N) linear scans appear four times across lookup helpers; a single `qualified_name → NodeId` index would fix all. |
| 17 | G3 | Hard-coded `/home/molaco/...` path in `spike_usages.rs` example. |
| 18 | G3 | Double-counting in spike's item walk (children + declarations). |
| 19 | G3 | `extract_usages` opens `sema.parse(file)` even when every ref in that file is an IMPORT (would be skipped). |
| 20 | G3 | Snapshot test (`17637c3e`) has no negative-case assertion that imports are filtered; no `usages_by_consumer` exercise; manual DUP_SORT iteration masks intent. |
| 21 | G4 | `pub extern crate` won't be flagged as an explicit pub-use. |
| 22 | G4 | Derive-macro-only usage of local traits could be falsely flagged dead-pub. |
| 23 | G6 | `75ee85c9` bundles four unrelated features (`forbidden_dependency_check`, `pub_use_pub_type_audit`, `re_export_chain`, `crate_dependency_metric`, `enum_variants`) under the attribute-extraction commit message. |
| 24 | G6 | No module-level attribute coverage. |
| 25 | G6 | No whitespace normalization for multi-line derives. |
| 26 | G6 | Substring-match semantics admit false positives from doc-comments. |
| 27 | G6 | Light test coverage: only `#[derive(...)]` exercised; `#[cfg]`/`#[must_use]`/enum-variant/assoc-item paths untested. |
| 28 | G7 | `FunctionSignature` omits `is_const`, `is_unsafe`, explicit lifetimes, where-clauses entirely. Will force another schema bump if consumers need them. |
| 29 | G7 | Tool descriptions in 3 places falsely advertise `OnceLock` init-fn stripping (only `LazyLock` is implemented). |
| 30 | G7 | `hir_trim` unbalanced-bracket safety net uses `tracing::trace!` (filtered by default) — silent miscount leaves no log trail. |
| 31 | G7 | `hir_trim` commit changes stored signature payload format without a schema bump. |
| 32 | G7 | `be099986` bundles hir_trim plus 4 unrelated wrapper changes (pagination, anchored attribute matching, sort/top_n on `crate_dependency_metric`, NodeId hex rendering, soft-fail manifest reader) in one ~1k-LOC change. |
| 33 | G8 | `spawn_blocking` offload applied inconsistently — only 2 tools wrapped while ~30 other LMDB-touching async handlers (incl. `mut_static_audit` from same group) still run synchronously on the runtime. |
| 34 | G9 | `cosine` silently truncates dim-mismatched vectors; only `embedder_version` string guards. |
| 35 | G9 | `max_cluster_size=15` makes the `max_pairs=50` member cap unreachable with defaults. |
| 36 | G9 | Cluster member truncation order is HashMap-iteration nondeterministic. |
| 37 | G9 | `similar_to_item` doesn't trim source while `semantic_overlaps` does — embedding queries diverge between the two tools. |
| 38 | G10a | `get_similar_code(target=<fn>)` in three places should be `query=<...>` (param is named `query`). |
| 39 | G10a | `e99fa6be` §23 cites `rust-guidelines-final.md` for section numbers, but that file is not in this repo (lives in sibling repos). |
| 40 | G10a | Stray `|` in one table cell at `workflows-detailed.md:919`. |
| 41 | G10b | All 5 Phase 8 audit tools (`missing_docs_audit`, `derive_audit`, `recursion_check`, `channel_capacity_audit`, `fn_body_audit`) documented in `TOOLS.md` but never referenced in `workflows.md`/`workflows-detailed.md`. |
| 42 | G10b | TOOLS.md architecture diagram says "21 graph tools" (actual: 36). |
| 43 | G10b | Stray `[ANCHOR](#channel_capacity_audit)` marker at `TOOLS.md:1461`. |
| 44 | G11 | `missing_docs_audit` should skip `#[doc(hidden)]` items. |

---

## Cross-cutting themes

- **Bundle-scope drift** (G6, G7) — commits include unrelated work not in the message.
- **Coverage-vs-docs gaps** (G5, G8, G10b) — tool descriptions promise more than the implementation delivers.
- **Schema discipline** is mostly good (v4→v9 bumps tracked) but G7 changes the stored signature payload without a bump.
- **Async hygiene** (G8) — `spawn_blocking` offload is incomplete; only 2 of ~30 LMDB-touching async handlers wrapped.
