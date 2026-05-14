# G1: Hypergraph Foundation (Phase 1–2) — Review

## 1. Group summary

These three commits stand up the workspace hypergraph end-to-end: a Cargo/rust-analyzer
loader that filters to local members, a HIR-driven extraction pass that materializes
Workspace/Crate/Module/Item/ExternalSymbol nodes plus typed Bindings (with namespaces,
provenance, and structured visibility), an LMDB/heed persistence layer with a SHA-256
content-addressed snapshot lifecycle (CURRENT pointer + manifest + atomic publish), a
small read-path API (`imports_of` / `exports_of` / `reexports_of` / `who_imports`), and
five MCP tools wired into the search-tool router. The third commit adds a re-export
facade traversal in `lookup_by_qualified_name` and a crate→root-module promotion in the
graph_tools resolver, both papering over collisions and aliases that the user-facing
qualified-name API would otherwise stumble on. Together they constitute the
prior-attempt rewrite that survives MDB_BAD_VALSIZE (proven by the burn-workspace
acceptance example) and forms the substrate the later commits build on.

## 2. Per-commit review

### `b1e4e0dc` — add graph module skeleton with workspace loader and heed dependency — 268 LOC

**What it does.** Adds `src/graph/{mod.rs, loader.rs}`, plumbs `pub mod graph` into
`src/lib.rs`, and pulls in `heed = "0.22.1"`, `ra_ap_base_db`, `ra_ap_hir_def`. The
loader wraps `ra_ap_load_cargo::load_workspace` with `no_deps = true`, then filters
`Crate::all(db)` down to the crates whose `root_file` matches a workspace-member
target's source path. Bundled with a `loads_self_workspace` test.

**Issues.**

- **minor** — `collect_member_roots` keys the local-crate filter on `target.root`
  (the `lib.rs`/`main.rs` path of each Cargo target). That's correct for `lib` and
  `bin` targets, but if a member package has only e.g. `examples` or non-standard
  target layouts, those targets would slip through. The classifier could end up
  with no roots and silently produce zero local crates. The diff does not assert on
  empty `local_crates` here (only the test does later).
- **nit** — `ProcMacroServerChoice::None` is fine for v1 but will under-count any
  items defined via proc-macros at extraction time. Worth noting in the module
  docs.

**Verdict.** Clean foundation. No blockers.

### `ad7743c3` — add persisted workspace hypergraph with bindings extraction, LMDB snapshots, and MCP tools — 2,561 LOC

**What it does.** The bulk of the work. Adds eight new modules:

- `ids.rs` — `NodeId`/`BindingId` as 32-byte SHA-256 of typed-component tuples
  with a `0x00` separator (collision-proof); serde via `serde_bytes`.
- `model.rs` — `ExtractionModel` (in-memory model), `Node` / `Binding` /
  `BindingVisibility` (Public / Crate / RestrictedTo / Private) / `BindingKind` /
  `Namespace` / `ItemKind` / `NodeKind`.
- `extract.rs` — Workspace + per-crate Crate node + per-module Module node
  + Contains edges. Module path computed by walking `def_map.containing_module`.
- `bindings.rs` — Per-`ItemScope` walk over `types()` and `values()`, emitting
  Item nodes for local declarations, ExternalSymbol stubs for non-local targets,
  and Binding records with Declared / NamedImport / GlobImport / ExternCrateImport
  kinds. Visibility encoded structurally via `encode_visibility`.
- `storage.rs` — `GraphPaths` filesystem layout, `GraphDatabases` heed
  schema (6 dbs, all hash-keyed under LMDB's 511-byte limit, 3 DUP_SORT
  secondary indexes), `compute_fingerprint` (SHA-256 over sorted .rs +
  Cargo.toml + Cargo.lock contents), manifest read/write.
- `snapshot.rs` — `build_and_persist` lifecycle (load → extract → write
  in single txn → manifest → atomic rename of `CURRENT`); `open_current` /
  `open_specific` open the published env.
- `queries.rs` — `lookup_by_qualified_name`, `imports_of`, `exports_of`,
  `reexports_of`, `who_imports`, plus visibility filter `is_visible_from`.
- `tools/graph_tools.rs` — five async MCP handlers + parameter structs in
  `search_tool.rs` + registration in `search_tool_router.rs`.

Plus an `examples/graph_burn.rs` acceptance binary against the burn workspace
that exposed the prior MDB_BAD_VALSIZE crash, and decent unit-test coverage of
each layer.

**Issues.**

- **major — Crate / root-module `qualified_name` collision.** In `extract.rs`,
  the Crate node and the *root* Module node are both emitted with
  `qualified_name = crate_name`. Their `NodeId`s differ (separate `"crate"` vs
  `"module"` kind labels), but `lookup_by_qualified_name` does a linear scan of
  `nodes_by_id` and returns the first match in hash-order. Calling
  `who_imports("file_search_mcp")` therefore returns either the Crate or the
  root Module non-deterministically; if it lands on the Crate, the result is
  empty (no binding ever targets a Crate node) instead of "all importers of the
  root module". This is what commit `e3b1666c` partially fixes — but only inside
  `resolve_required_node` for `expect_kind == Module`. The collision is
  inherent to the extraction model.
- **major — Duplicate bindings for ADTs across namespaces.** Unit/tuple
  structs and unit enum variants live in BOTH the type and value namespaces.
  `extract_bindings` iterates `item_scope.types()` and `item_scope.values()`
  independently and emits one Binding for each, sharing `from_module`,
  `visible_name`, `target`, and `kind`, differing only by `namespace`.
  Downstream queries (`imports_of` / `who_imports` / `exports_of`) make no
  attempt to dedup or filter by namespace, so users see each such re-export
  twice. The codebase has since added a post-hoc retain-by-(from_module,
  visible_name, target, kind) dedup in `bindings.rs`, but that was not in
  this commit.
- **minor — `compute_fingerprint` exclusion is path-component-based, not
  root-anchored.** `c.as_os_str() == "target" || ".git"` skips *any* directory
  named "target" or ".git" anywhere in the tree (e.g., `src/target/foo.rs`).
  Probably benign in practice but slightly surprising.
- **minor — `write_model` writes Binding records keyed by a `BindingId` whose
  composition is `(from_module_hex, "T"|"V", visible_name, target_hex)`** —
  collision-free across the same module, but two bindings differing only in
  `kind` (Declared vs Glob, same from_module / name / target — pathological
  but possible if HIR reports a glob shadow of a declaration) would collide
  and the second `put` overwrites the first. The `BindingKind` is not in the
  `BindingId` components.
- **minor — `enrich_bindings` silently swallows `read_txn` errors** and
  returns an empty list. The MCP caller has no way to distinguish "no
  bindings" from "transient LMDB error". Use `?` or surface the error.
- **minor — `OpenedSnapshot::write_txn`** is public but unused outside this
  file. It hands callers a way to mutate a published snapshot under their
  feet. Probably an unused escape hatch; should be `pub(crate)` at most.
- **minor — `SCHEMA_VERSION = 1` with `read_manifest` `bail!` on mismatch**,
  but no migration story documented. Fine for v1, worth a TODO.
- **minor — `compute_fingerprint` reads every .rs file in the workspace
  serially**, which on burn-scale (thousands of files) takes noticeable
  wall-time on every `build_and_persist` call; the reuse fast-path is in
  fact gated on this fingerprint so cache hits are not free. Worth
  parallelizing with `rayon` later.
- **nit — `_binding_id_marker(_: BindingId)` and `_path_marker(_: &Path)`** —
  dead-code-warning silencers for unused imports. Cleaner to either use the
  import or `#[allow(unused_imports)]`.
- **nit — `module_def_owner_module` does not handle `ImplId` / `MacroId`**,
  but those are filtered upstream — so it's defensively `None` and harmless,
  just worth a comment.
- **nit — `who_imports` does NOT filter by `kind != Declared`.** Re-reading:
  it does — `if binding.kind != BindingKind::Declared { out.push(binding); }`.
  Disregard.
- **nit — The `_explicitness` field on `HirVisibility::Module(..)` is ignored.**
  This loses the syntactic-vs-inferred distinction. Acceptable for v1; commit
  `e3b1666c`-and-later adds `is_explicit_pub_use` to recover this for
  `Import`-flavored bindings but not for `Declared`/module-restricted ones.
- **nit — `loader::load`'s `Default::default()` argument to `load_workspace`**
  is `&load_cargo::ProgressFn` (or similar) — opaque, fine, but the load
  produces no progress diagnostics for what can be a multi-minute build.

**Verdict.** The architecture is sound and the layering is exemplary, but the
two `major`-tagged issues are real user-visible bugs that the next commit only
half-addresses for the crate/module collision and leaves entirely open for the
namespace duplicates.

### `e3b1666c` — add re-export facade and crate-to-root-module fallbacks for qualified name lookup — 679 LOC

**What it does.** Two user-facing fixes plus two debug examples:

1. **Re-export facade fallback** in `lookup_by_qualified_name`: when the
   canonical-name scan misses, split on the last `::`, recursively resolve
   the prefix, then look for a non-Declared binding in that module whose
   `visible_name == leaf` and follow its `target`. Bounded by
   `MAX_REEXPORT_HOPS = 8` to prevent runaway cycles. Three new tests cover
   the facade path, the canonical path, and the unresolvable path.
2. **Crate→root-module promotion** in `graph_tools::resolve_required_node`:
   if the user supplied a Crate qualified name where a Module was expected,
   transparently fall through to `find_root_module_of(crate_id)` (a new
   helper). Plus a `get_exports_accepts_crate_name_as_consumer` regression
   test and a shared `DEFAULT_SNAPSHOT_LOCK` mutex around the two tests
   that open the default-data-dir snapshot in-process.

Two examples — `examples/debug_burn_loader.rs` and `examples/debug_burn_target.rs` —
are forensic dumpers that hardcode `/home/molaco/Documents/burn` as default and
parse `Cargo.toml` by hand to compare against the loader's view.

**Issues.**

- **minor — Phase 2 fallback never triggers when Phase 1 returns the WRONG
  match.** The Phase 1 scan walks all nodes and returns the first node
  whose `qualified_name == name`. For an ambiguous name (`file_search_mcp`,
  matching both Crate and root Module), Phase 1 picks one of them and Phase
  2 never runs. Whether the caller gets the "right" answer depends on
  iteration order. The fix in `resolve_required_node` papers over this for
  `expect_kind == Module`, but `who_imports` calls `lookup_by_qualified_name`
  *without* the wrapper, so `who_imports("file_search_mcp")` can still
  silently return an empty list because it resolved to the Crate node.
- **minor — Performance: Phase 2 recursion does an O(N) `nodes_by_id`
  iterator scan per hop.** With `MAX_REEXPORT_HOPS = 8` and N ≈ 40K on
  burn-scale that is up to ~320K node reads per lookup. The comment in
  `mod.rs` claims "sub-millisecond at burn scale" but that's the Phase 1
  measurement; recurrent facades on a cold OS-page-cache may be markedly
  slower. Building a `qualified_name → NodeId` index in `nodes_by_id`
  schema would eliminate this entirely.
- **minor — `find_root_module_of` does yet another full `nodes_by_id` scan.**
  Per call. For a 5-line helper invoked from a hot resolver path, this is
  wasteful — a tiny `roots_by_crate: NodeId → NodeId` secondary index in
  storage would be a one-line shoehorn.
- **minor — `is_explicit_pub_use` field is on `Binding`** in the current
  model.rs but is NOT in the original commit `ad7743c3` Binding shape. The
  diff for `e3b1666c` against `queries.rs` does not introduce it either —
  it's from a *later* commit. The cross-commit ordering is therefore fine,
  but readers should be aware that `model.rs` shown by Read does not match
  what was on disk after `e3b1666c`.
- **nit — `debug_burn_loader.rs` parses `Cargo.toml` by hand** with a
  regex-free `find` / `strip_prefix` dance. It will mis-handle
  `[workspace.metadata.foo]` tables, inline-table members, or any TOML
  using single-line array notation across lines. Bug-prone — a `toml`
  crate one-liner would be safer — but the file is an examples/debug
  utility, not production code.
- **nit — Both debug examples hardcode `/home/molaco/Documents/burn` as a
  default**. Personal-machine paths in version-controlled examples are a
  smell, even if overridable via argv.
- **nit — `mcp_round_trip_against_self` and `get_exports_accepts_crate_name_as_consumer`
  share a `static Mutex` because heed forbids opening the same env twice in
  one process.** The comment explains it, but a `tempfile::tempdir`-scoped
  `data_dir_override` per test (as the in-`graph::queries` tests already do)
  would dodge the need for the lock and make the suite trivially parallel.

**Verdict.** The fixes are correct and well-tested for the cases they cover.
The crate-name ambiguity at `who_imports` and the O(N²) cost of the lookup
helpers are the leftover sharp edges.

## 3. Cross-commit observations

- **Layering is exemplary.** Each commit is a coherent slice: skeleton →
  full system → user-visible polish. Each layer has its own tests, mostly
  hermetic via `tempfile::tempdir` + `data_dir_override`.
- **Crate / root-module collision is consistent across commits.** Commit 2
  introduces it; commit 3 acknowledges it (the doc-comment on
  `find_root_module_of` explicitly names it); the chosen fix is local
  (transparent promotion at one resolver call site) rather than structural
  (giving the root module a distinct `qualified_name` or returning Vec<>
  from the lookup). This leaks: any future tool that calls
  `lookup_by_qualified_name` outside `resolve_required_node` will hit the
  same ambiguity. The `who_imports` MCP handler is the existing example.
- **Duplicate-binding issue across namespaces.** Commit 2's bindings pass
  emits two bindings per ADT (one per namespace). Commit 3 does NOT address
  this. The retain-based dedup visible in today's `bindings.rs` is a
  later patch — within this G1 group, callers of `imports_of` /
  `who_imports` against an ADT will see duplicates.
- **Linear-scan performance shows up four times** (Phase 1 lookup,
  Phase 2 recursion, `find_root_module_of`, and `lookup_by_qualified_name`
  itself). Adding a single `qualified_name → NodeId` LMDB index would
  collapse all four to O(log N). Worth opening a follow-up.
- **Error handling at the MCP boundary is uneven.** `enrich_bindings` silently
  swallows `read_txn` errors; `resolve_required_node` and
  `open_workspace_snapshot` correctly surface them. Tighten in a follow-up.
- **Determinism / hashing.** NodeId composition (kind + crate + module
  path + item kind + item name + workspace hash + separators) is correct
  and stable across rust-analyzer reloads. BindingId composition omits
  `BindingKind` — slightly weaker than NodeId in pathological cases (see
  minor issue in commit 2) but not exploitable in normal Rust code.
- **No security-relevant surface.** No command execution, no untrusted
  deserialization on a network boundary; everything is local filesystem
  + heed. Path handling uses `canonicalize` correctly.

## 4. Overall verdict

**MINOR — small issues but ship-able.**

The architecture is solid, the layering is clean, and the tests cover the
happy paths plus a few key regressions. The two `major` issues identified in
commit 2 (crate/root-module collision; duplicate-bindings for ADTs across
namespaces) are user-visible but documented (the first) and self-limiting (the
second — only re-exports of unit structs/variants double-count). Neither is a
crash or a data-loss bug. The remainder are performance and ergonomic cleanups
that the codebase clearly intends to iterate on. Land it; track the leftover
items in follow-up issues — especially a `qualified_name → NodeId` secondary
index that eliminates four separate full-table scans.
