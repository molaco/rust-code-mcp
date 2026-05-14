# G3 — Usage Extraction Pass — Review Report

## 1. Group summary

G3 introduces a non-import usage-reference pass over the workspace hypergraph. Commit `00c17be9` lands two example binaries: a phase-0 timing spike for `Definition::usages(sema).all()` and a small item-count probe, both used to size the eager-extraction cost before committing schema. Commit `076ef894` widens the model and storage schema for `Usage` records (new `Usage` / `UsageCategory` types, `UsageId` hash, three new LMDB sub-DBs `usages_by_id` / `usages_by_target` / `usages_by_consumer`, `usage_count` field on `BuildResult` and `GraphManifest`, and `SCHEMA_VERSION` bumped to 12). Commit `70609bac` adds the actual extraction (`src/graph/usages.rs::extract_usages`), wires it after `extract_bindings`, threads the returned `def_to_node` map, and dumps usage-by-category counts in `examples/count_items.rs`. Commit `17637c3e` adds three snapshot-test assertions (non-zero `usage_count`, at least one `usages_by_target` row for `load`, and field-level sanity on a single `usages_by_id` record). The four commits are correctly ordered: model+storage land before the extraction pass that populates them, and tests land last. The `Usage` shape constructed by the extraction matches the struct defined in 076ef894 exactly (6 fields, `consumer_function` arrives much later in v6).

## 2. Per-commit review

### `00c17be9` — add spike_usages and count_items examples for usage-pass cost analysis — 142 LOC

What it does
- Adds `examples/spike_usages.rs`: walks every reachable `ModuleDef` in every local crate, takes a strided sample of size N, and times `Definition::usages(sema).all()` per item, printing totals and per-item averages. Default workspace is hard-coded to `/home/molaco/Documents/coding-agent` — fine for a throwaway spike but a smell if left in tree.
- Adds `examples/count_items.rs`: builds the workspace, opens current snapshot, prints a `BTreeMap<NodeKind, count>` histogram.
- Adds two `[[example]]` entries to `Cargo.toml`.

Issues
- (Low) `spike_usages.rs` walks both `module.children(db)` and `module.declarations(db)`, double-counting items reachable through inline submodules vs siblings (children pushes sub-modules; declarations enumerates items including those sub-modules' parents). For a spike that just needs an order-of-magnitude this is fine, but the printed "local items reachable from root modules" number overstates true item count.
- (Low) The hard-coded default path (`/home/molaco/Documents/coding-agent`) leaks the author's machine layout into a committed example. Replace with `unwrap_or_else(|| eprintln!("usage: ..."); std::process::exit(2))` or take it as a required arg.
- (Trivial) `truncate` panics if `max == 0`; harmless for the call site (`50`) but a foot-gun.

Verdict: ACCEPT — diagnostic-only code that justifies the design decision in the next commit. Hard-coded path should be cleaned up before this code outlives its purpose.

### `076ef894` — add Usage model and storage for non-import references — 146 LOC

What it does
- Adds `UsageCategory { Read, Write, Test, Other }` and `Usage { target, consumer_module, file, start, end, category }` in `model.rs`.
- Adds `UsageId([u8; 32])` newtype in `ids.rs` matching the existing `BindingId` shape (sha256, hex display, 12-char debug truncation).
- Adds `usages_by_id` (Bytes → SerdeBincode<Usage>), `usages_by_target` (NodeId → UsageId, DUP_SORT), `usages_by_consumer` (NodeId → UsageId, DUP_SORT) sub-DBs in `storage.rs`. Both `create` and `open` paths populated.
- Adds `usage_count: u64` (with `#[serde(default)]`) to `GraphManifest` and propagates it through `BuildResult`, `write_model`, the manifest meta-key `"usage_count"`, etc.
- Bumps `SCHEMA_VERSION` from 11 to 12 with a comment block explaining v1/v2 disjoint graph-ids.
- Adds `usage_id_for(&Usage)` helper that hashes target/consumer/file/start/end/category-tag. Inserts iterate `&model.usages` and push to all three sub-DBs.

Issues
- (Medium — efficiency) `UsageId` keys are 32-byte sha256 hashes whose only role is to give DUP_SORT an opaque non-colliding value. The id encodes `target | consumer | file | start | end | category` — but `usages_by_id` is then keyed *only* by that hash and the entire `Usage` (which redundantly carries `target`, `consumer`, `file`, `start`, `end`, `category`) is stored as the value. With 5.2k usages on coding-agent that's ~5.2k × ~80 bytes serialized payload plus 5.2k × 32-byte keys, twice (once in `usages_by_id`, once duplicated across the two index DBs). A composite key like `target ++ consumer ++ start_le_u32` would be smaller and skip the hash. Not a blocker — bindings already use the same pattern — but worth noting that the schema cost grows linearly with refs (the largest table in the graph by row count after this lands).
- (Low — efficiency) `usage_id_for` calls `to_hex()` on `target` and `consumer` to build the components, then converts ints to decimal strings; `UsageId::from_components` then re-hashes all of that. Hashing the raw 32-byte `NodeId.as_bytes()` + `start.to_le_bytes()` would be faster and deterministic. Hot path is bounded (~5k items workspace-scale), so this is a latent concern not an issue today.
- (Low — schema bug surface) `usages_by_id` uses `Bytes → SerdeBincode<Usage>` while `usages_by_target` / `usages_by_consumer` use `Bytes → Bytes` (with manual DUP_SORT). That's consistent with the bindings tables, but means the test in 17637c3e that iterates `usages_by_target` to find a key match (manual `k == load_fn_id.as_bytes()`) cannot use a point lookup; with DUP_SORT and the right API it could use `get_duplicates` (or `prefix_iter`) but the test scans every row. Not introduced by this commit, but the layout makes the test verbose.
- (Low — manifest compat) `#[serde(default)]` on `usage_count` means a v1 JSON without the field deserializes to `0`. Combined with `graph_id_for` hashing `SCHEMA_VERSION` (which makes v1/v2 graph_ids disjoint), this is safe in practice — v1 snapshots are never loaded by v2 readers — but the `default` provides a reasonable fallback if someone ever hand-edits a manifest. Acceptable.
- (Style nit) The two index DBs are declared as `Database<Bytes, Bytes>` with the value-comment "NodeId → UsageId". That's correct, but `UsageId` is opaque here — anything that fits in 32 bytes works. The doc comment "v2 (2026-05): added usages_by_id / usages_by_target / usages_by_consumer sub-databases" on the version block is good; would be even better paired with a one-line schema row layout (`key=NodeId(32), value=UsageId(32)`).
- (Trivial) `serde_bytes_32` is referenced in `#[serde(with = "...")]` on `UsageId.0`; the helper module is the same one `BindingId` and `NodeId` already use, so no new wire-format risk.

Verdict: ACCEPT. Schema is consistent with the existing bindings layout and the migration story (`SCHEMA_VERSION` bump + `#[serde(default)]`) is the same pattern the codebase has used for prior bumps. The efficiency notes are observations about scaling, not blockers.

### `70609bac` — add usage extraction pass for non-import item references — 179 LOC

What it does
- New `src/graph/usages.rs::extract_usages(model, db, vfs, def_to_node, module_node_for)`. Walks `def_to_node`, filters to nodes whose kind is `Item`, converts `ModuleDefId → Definition`, runs `Definition::usages(sema).all()`, and for each non-`IMPORT` reference resolves its file path (workspace-relative or skip), looks up its enclosing module via `sema.scope_at_offset(...).module()`, and pushes a `Usage` record.
- Wires the new pass into `extract.rs` after `extract_bindings`; modifies `extract_bindings` to return `HashMap<ModuleDefId, NodeId>` so the pass can reuse it without a second walk.
- Updates `examples/count_items.rs` to dump usage-by-category counts and to call `BuildOptions { force_rebuild: true, .. }` so the example timing is meaningful.

Correctness review

- **Import-filter is conservative-correct.** `r.category.contains(ReferenceCategory::IMPORT)` strips references whose category includes the IMPORT flag, which RA sets on the path component of a `use` statement that resolves to the target. This is the right filter — those are already modeled as `Binding`s. Bitflag semantics mean a reference flagged `IMPORT | READ` is still skipped, which is correct (it's still inside a `use`).
- **Alias filter — gap.** A `use foo::Bar as Baz;` resolves `Bar` to `Definition::Adt(Foo::Bar)` and RA tags the path-component with IMPORT. That's handled. But `pub use foo::Bar as Baz;` followed by a downstream `Baz::method()` will be picked up by `Definition::usages(Foo::Bar)` because RA resolves `Baz` back to its underlying def — that's the *desired* behavior (we want consumers via aliases counted), and the file/range hits the consumer site, not the alias declaration. Verified by reading the test fixture intent — though no test in G3 directly exercises a re-export alias. Worth a follow-up test.
- **Local-only filter.** `def_to_node` contains entries for both local items (NodeKind::Item) and external stubs (NodeKind::ExternalSymbol) plus modules. The pass filters with `if node.kind != NodeKind::Item { continue }` which is correct: modules and externals are skipped. Good — the doc comment explains this.
- **Reference-site filter.** Refs in workspace-local files survive `resolve_workspace_relative(...)`; refs in dep crates produce `None` and are skipped. Refs inside macro expansions that RA reports against an in-workspace file are surfaced as Usages; refs that land in dep-crate files are not. That's consistent with the rest of the extraction.
- **Consumer module attribution.** `sema.scope_at_offset(syntax, r.range.start()).module()` returns the module that lexically contains the reference. This handles the inline-`mod`-in-the-same-file case correctly (different submodule scopes within one file map to different `ModuleId`s). When this lookup fails (e.g. macro-generated tokens with no file scope), the reference is skipped — fail-safe.
- **`module_node_for` gating.** `module_node_for.get(&consumer_module)` returning `None` filters out refs whose module is not workspace-local (e.g. resolved into a dep crate's module). Correct.
- **`source = sema.parse(*ed_file_id)` per file.** `parse` is called once per `(target, file)` pair — not per ref — which is fine. For a hot target with many refs across many files this is still N(files) parses per target, but RA caches and the spike measured ~1.3 ms/item average end-to-end.
- **`workspace_root` clone is cheap.** Copied once per call into the closure, used for prefix-stripping. Fine.

Performance

- Reported ~1.3 ms/item, ~1.4 s total on coding-agent (1087 items, 5.2k refs). That's consistent with the spike timing in 00c17be9 and is added to the *extraction* (one-time, cached). Acceptable. The scaling concern is linear in items × avg-refs-per-item, both bounded by workspace size; no quadratic blowup.
- The pass holds `Semantics::new(db)` for the whole walk inside `attach_db`. Calling `def.usages(&sema)` requires the salsa db attachment; this is the canonical pattern from RA itself.
- One subtle missed optimisation: `results.references` is iterated and we open `sema.parse(*ed_file_id)` for each file even when the only reference in that file is an IMPORT we'll skip. For a heavily re-exported root-module item that's a lot of wasted parses. Not a blocker — the IMPORT-only case has zero remaining refs so the inner loop is cheap — but a `refs.iter().any(|r| !r.category.contains(IMPORT))` pre-check before `sema.parse` would shave time.

API match across commits

- 076ef894 defined `Usage { target, consumer_module, file, start, end, category }`. 70609bac constructs exactly that struct. No `consumer_function` is present yet (added in a later v6 commit outside this group). Cross-commit ordering is sound.

Other issues

- (Low) `examples/count_items.rs` now forces a rebuild (`force_rebuild: true`) on every invocation. Useful when measuring usages but slows down the "just count" use-case. A flag would be nicer; for an example binary this is fine.
- (Trivial) `resolve_workspace_relative` boxes the `Utf8Path → PathBuf` conversion via `vfs_path.as_path()?.to_path_buf().into()`. Acceptable.

Verdict: ACCEPT. Correct filtering on imports, dep-crate refs, and non-Item nodes; module attribution is right for inline-`mod` cases; the API matches the model commit; the performance number is reasonable.

### `17637c3e` — add snapshot test assertions for usage extraction — 37 LOC

What it does
- Adds three assertions to the existing `build_and_persist_smoke_test` (or equivalent):
  1. `result.usage_count > 0` — at least one usage was persisted.
  2. Iterating `opened.dbs.usages_by_target`, count rows where the key equals `load_fn_id.as_bytes()`; assert ≥1.
  3. Take the first record from `usages_by_id` and assert `start <= end`, `file != ""`, `!file.starts_with('/')` (workspace-relative invariant).

Issues
- (Medium — shallow coverage) The test is presence-only. It does not verify:
  - That an import-target reference *isn't* in `usages_by_target` (the whole point of the import filter).
  - That `category` classification is correct (Read vs Write vs Test).
  - That `consumer_module` resolves to the expected NodeId.
  - That the `usages_by_consumer` index has matching entries for the consumer side of the same reference (i.e. the two indexes agree).
  - That iterating all DUP_SORT duplicates for `load_fn_id` returns more than the single hit; the manual `iter` + `k == ...` filter visits every key, which works but is the wrong way to use a DUP_SORT index (`get_duplicates(k)` would be both clearer and faster). The G3-era code may not have a helper for that yet, but the comment "Just assert ≥1 usage" undersells the test.
- (Low — fragility) Taking `usages_by_id.iter().next()` and inspecting *one* record is a weak invariant — any future change to BTreeMap-ordering or sub-DB key ordering could land a different first row. Sane fields (start ≤ end, non-empty file, no leading slash) are true of *every* row by construction, so a `for entry in iter { ... }` checking all rows would be both stronger and equally cheap.
- (Low — `usages_by_consumer` never asserted) The third sub-DB introduced in 076ef894 is not exercised at all. If the producer loop in `snapshot.rs::write_model` ever skipped the `dbs.usages_by_consumer.put(...)` call by mistake, no test in this group would catch it.

Verdict: MINOR — test coverage is real but shallow. A second test that asserts (a) a known import is *not* recorded, (b) classification matches the call kind, and (c) `usages_by_consumer` is non-empty would close the obvious gaps. The shipped assertions still meaningfully guard against the most catastrophic regression (zero rows written).

## 3. Cross-commit observations

- **Ordering is correct.** Model & storage (076ef894) precede the extraction pass (70609bac), which precedes the tests (17637c3e). The signature of `Usage` doesn't change between 076ef894 and 70609bac — `consumer_function` arrives in a later v6 commit outside this group.
- **Schema-version bump is consistent.** 076ef894 bumps `SCHEMA_VERSION` to 12 and documents v1/v2 disjoint graph_ids. `graph_id_for` hashes `SCHEMA_VERSION`, so v1 snapshots on disk are automatically ignored. Older snapshots are not deleted but won't be reused — same migration pattern the rest of the codebase uses.
- **`extract_bindings` signature change is the only public-API ripple.** It now returns `HashMap<ModuleDefId, NodeId>` instead of `()`. This is internal (the function is only called from `extract.rs`) so no external callers break.
- **Spike-to-implementation feedback loop is visible.** The reported ~1.3 ms/item in `usages.rs`'s module doc matches the spike's expected output; this is a healthy "measured the cost, decided eager extraction is fine, kept the spike binary for future re-measurement" pattern.
- **DUP_SORT index iteration smell.** Three places now iterate a DUP_SORT sub-DB and manually filter by key (bindings test + usages test). A `get_duplicates(txn, &key)` helper on `GraphDatabases` would let tests express intent and reduce O(n) scans. Not introduced by G3 — but G3 propagates the pattern.
- **No `usages_by_consumer` exercise.** Of the three new sub-DBs, two are touched by the test (`usages_by_id` and `usages_by_target`) and one (`usages_by_consumer`) is not. Easy follow-up.
- **Example hygiene.** The hard-coded `/home/molaco/...` default in `examples/spike_usages.rs` is a smell that should be cleaned before this code is shared more widely; the `force_rebuild: true` flip in `examples/count_items.rs` reduces the example's usefulness as a quick sanity check.

## 4. Overall verdict

**PASS with MINOR cleanups.**

The schema is consistent, the extraction is correct on the cases it claims to cover (non-import refs, local items only, inline-`mod` attribution), the performance number is grounded by an actual spike, and the cross-commit API contract holds. The test coverage is the weak spot — three assertions that prove "something was written" rather than "the right thing was written" — but doesn't block merge. Suggested follow-ups (not blockers):

1. Add a test that asserts an `IMPORT`-category ref is *not* materialized as a Usage.
2. Add a test that asserts `usages_by_consumer` has at least one row and that for some `(target, consumer)` pair both indexes agree.
3. Add a `get_duplicates` helper on `GraphDatabases` and rewrite the manual-iter tests against it.
4. Replace the hard-coded path in `examples/spike_usages.rs` with a required CLI arg or a `CARGO_MANIFEST_DIR`-relative default.
5. (Optional) Pre-filter `refs` for non-IMPORT entries before calling `sema.parse(file)` in `usages.rs` to avoid wasted parses on import-only files.
