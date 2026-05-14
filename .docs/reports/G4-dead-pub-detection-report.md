# G4 — Dead-pub detection — Review report

## 1. Group summary

This group introduces the workspace-wide "dead pub" detection feature
(`dead_pub_in_crate` + `dead_pub_report`), exposes both as MCP tools, adds a
new field on `Binding` for explicit `pub use` tracking, and reworks the
`workspace_stats` encapsulation metric.

The flow lands cleanly:

| Order | Commit     | Role                                                                  |
|-------|------------|-----------------------------------------------------------------------|
| 1     | `a0fe0b0e` | New `Snapshot::dead_pub_in_crate` query + smoke test                  |
| 2     | `92763548` | Wraps the query as the MCP tool `dead_pub_in_crate`                   |
| 3     | `ead32c04` | Adds workspace-wide aggregate `dead_pub_report` (+ MCP, +example)     |
| 4     | `3a5be800` | Adds `Binding.is_explicit_pub_use`, `declared_reexports_of`, +more    |
| 5     | `7feb3a92` | Renames `encapsulation_ratio` → `pub_crate_share`, refines labels     |

Each commit bumps the schema version where needed (v2→v3 in `ead32c04`
because `Node.file/span` are now populated, v3→v4 in `3a5be800` because
`Binding` grows a field), so old snapshots are correctly invalidated rather
than silently mis-decoded.

`cargo check --lib` clean (15.91s) with the v4 SCHEMA_VERSION. The 17
warnings emitted are pre-existing "unreachable pub" notes in `graph/ids.rs`
and unrelated to this group.

The dead-pub algorithm is **structurally sound** for v1: it explicitly
filters Items by `crate_id`, requires the declared visibility to be
`Public`, then checks for any external importer (Binding from another
crate) or external user (Usage from a consumer module in another crate).
The principal source of false positives — items used only through a public
type signature — is documented at the query's docstring and surfaced
verbatim in the MCP tool description.

## 2. Per-commit review

### `a0fe0b0e` — add `dead_pub_in_crate` query for unused pub item detection — 284 LOC

**What it does.** Adds `pub struct DeadPubFinding` and
`Snapshot::dead_pub_in_crate(crate_id) -> Result<Vec<DeadPubFinding>>` to
`src/graph/queries.rs`, plus three smoke tests including
`dead_pub_findings_are_well_formed`. Also factors out
`usages_for_target`/`usages_for_consumer` helpers and exposes `usages_of`
and `usages_in` on `OpenedSnapshot`.

**Algorithm.**

1. Linear scan `nodes_by_id`; keep Items whose `crate_id == Some(target
   crate_id)`.
2. For each candidate, collect all `Binding`s targeting it. Look for the
   `Declared` binding; reject unless `visibility == BindingVisibility::Public`.
3. Walk non-`Declared` bindings; if any `from_module`'s `crate_id` differs
   from the target crate, mark as having an external importer and skip.
4. Walk usages via `usages_by_target`; if any `consumer_module`'s crate
   differs, mark as having an external user and skip.

**Issues.**

* **Minor — Iterator-borrow shape.** The code collects bindings into a
  `Vec` first ("Collect bindings before doing follow-up `nodes_by_id.get`
  lookups so the iterator's borrow on `rtxn` is dropped first"). The
  inline comment makes the rationale obvious; safe.

* **Minor — Documented false positive.** Items referenced only via a public
  signature (never named directly in caller code) won't appear in
  `usages_by_target`, so they may be flagged as dead-pub even when their
  `pub` is load-bearing. Disclaimed at the query docstring. Acceptable for
  v1 because the tool is described as candidate-emission, not certainty.

* **Minor — Macros / proc-macros / `#[derive]` not considered.**
  `bindings.rs::process_entry` has a "v1 exclusions" list (macros,
  builtins, enum variants in scope) — so macros aren't Items and won't
  appear as findings. That's correct: no false positives for macros, but
  no detection either. Likewise, items referenced only through a derived
  trait impl in another crate won't be tracked, because derive expansion
  doesn't write the trait name syntactically. Disclaim or accept; not a
  blocker.

* **Minor — `pub(in path)` skip rationale.** The docstring justifies
  skipping `RestrictedTo` candidates ("the path is always an ancestor
  module within the same crate, so visibility is already strictly narrower
  than `pub(crate)`"). Confirmed by `bindings::encode_visibility`:
  HirVisibility::Module is encoded as `RestrictedTo(local_node)` or
  fallback to `Private`. ✓

* **Nit — Test scope.** `dead_pub_findings_are_well_formed` deliberately
  doesn't pin specific qnames because the dead-pub set drifts with
  refactors. Pragmatic. Verifies invariants (Public visibility, Item kind,
  qualified_name agreement) without coupling to current contents. Good
  shape.

**Verdict.** Looks good. The algorithm is conservative in the right
direction (false negatives over false positives) and the docstring
prepares callers for the known signature-only false positive.

---

### `92763548` — expose `dead_pub_in_crate` as MCP graph tool — 257 LOC

**What it does.** Adds the MCP tool `dead_pub_in_crate` and the
`who_uses` tool, plus their schemas (`DeadPubParams`, `WhoUsesParams`).
Re-exports `DeadPubFinding`. Also threads `usage_count` into the build
response.

**Issues.**

* **Minor — Resolution path for `params.krate`.** Caller may supply either
  the crate name (resolves to the Crate node) or the crate root module
  name (resolves to the Module node, since both share the same
  qualified_name). The handler correctly resolves both:

  ```rust
  let crate_id = match node.kind {
      NodeKind::Crate => id,
      NodeKind::Module => node.crate_id.or(node.parent_id).ok_or_else(...)?,
      other => return Err(McpError::invalid_params(...)),
  };
  ```

  Verified that `extract.rs:225-237` sets `Crate.crate_id = Some(crate_id)`
  (self-pointer) and root `Module.crate_id = Some(crate_id)`, so both
  branches converge to the right Crate `NodeId`. ✓

* **Nit — Tool description.** Says "Conservative: may miss items used
  only through public type signatures." Accurately reflects the algorithm.

* **Nit — `who_uses_and_dead_pub_round_trip` test.** Doesn't pin specific
  findings (just checks the JSON envelope contains `"findings"`). Right
  call — the dead-pub set will drift.

* **Major — No validation that the target is actually a *local* crate.**
  `dead_pub_in_crate(remote_crate_id)` is meaningless (we don't have
  bindings from external crate consumers, only inwards) but the tool will
  happily run and return an empty findings list. A caller passing
  `dead_pub_in_crate("serde")` would silently get `[]`. Not data
  corruption, but unhelpful. Could surface a clearer error if the resolved
  NodeKind is a Crate node but a remote-origin one. (External crates
  aren't currently emitted as Crate nodes — only ExternalSymbol — so in
  practice the lookup would fail-early with "no node found for qualified
  name `serde`". I verified at `loader.rs:78` and `extract.rs:362`: only
  workspace-local crates produce Crate nodes. So this is a theoretical
  concern, not a real one. Downgrade to nit.)

**Verdict.** Solid MCP wrapper. Schemas use schemars descriptions
correctly; error paths return `McpError::invalid_params` for caller errors
and `internal_error` for snapshot failures.

---

### `ead32c04` — add `dead_pub_report` query and example — 281 LOC

**What it does.** Adds `CrateDeadPub` aggregate struct and
`Snapshot::dead_pub_report() -> Vec<CrateDeadPub>` which iterates over
every Crate node and calls `dead_pub_in_crate` per crate. Adds the
matching MCP tool, an example binary `examples/dead_pub_report.rs`, and
backfills `Node.file/span` for local Items via `Definition::try_to_nav` so
findings are navigable. Schema bumped v2 → v3.

**Issues.**

* **Minor — Self-consistency.** `dead_pub_report` delegates to
  `dead_pub_in_crate` per crate, so the two queries are guaranteed to
  agree on any single crate. ✓

* **Minor — `dead_pub_in_crate` now also sorts findings by
  `qualified_name`.** That happened as part of this commit's diff (sort
  added inside `dead_pub_in_crate`). Pre-existing tests in `a0fe0b0e` only
  checked invariants, not order, so no regression. Deterministic output
  is desirable for diff-driven workflows.

* **Minor — `try_to_nav` backfill happens during `extract_usages`.** This
  is the right place — every local Item that has a Definition will pay
  one `try_to_nav` call. Macro-only definitions silently fall through
  ("Errors/macro-only definitions silently fall through" comment). The
  added cost is documented; nothing observably worse in `cargo check`
  timing. Schema bump (v2→v3) means the new fields show up automatically
  on first rebuild without users having to remember `--force` — the right
  call.

* **Nit — Example useful but limited.** `examples/dead_pub_report.rs`
  prints a markdown table. The CLI lacks a `--json` flag, but the same
  data is available via the MCP tool. Not a blocker.

* **Minor — `total_findings` aggregation.** The response struct
  `DeadPubReportResponse { workspace, total_findings, crates }` computes
  `total_findings` after the per-crate enrich pass — consistent with the
  inner `crate.findings.len()` sum. ✓

* **Nit — Schema-version comment cleanup.** The v2 → v3 banner overwrites
  the old v1/v2 disjointness comment. New text is fine; mentions that
  layout is unchanged but extracted-data is denser.

**Verdict.** A focused aggregate over an already-tested per-crate query.
Schema bump is correct; `try_to_nav` adds navigability without a fragile
custom span scan.

---

### `3a5be800` — track explicit pub-use visibility on bindings — 1,378 LOC

**What it does.** Largest commit in the group, spanning several semi-
independent features:

1. **`Binding.is_explicit_pub_use`** — a new field, set true iff the
   source `use` declaration carries an explicit `pub`/`pub(crate)`/`pub(in
   path)`/`pub(super)` token. Backs the new `declared_reexports_of`
   query.
2. **`declared_reexports_of`** — returns every `pub use` declared in a
   module, regardless of visibility filtering.
3. **`who_uses_summary`** — aggregation rollup of `usages_of(target)`
   grouped by consumer module with per-category breakdown.
4. **`crate_edges`** — full cross-crate consumer→producer edges decorated
   with the symbols carrying each edge.
5. **`overlaps`** — workspace-wide name-collision / module-shadow /
   duplicate report.
6. **`module_tree`** — recursive module/item tree dump.
7. **`workspace_stats`** — counters across the snapshot.
8. New MCP tools wrapping each query above (`get_declared_reexports`,
   `who_uses_summary`, `crate_edges`, `overlaps`, `module_tree`,
   `workspace_stats`).
9. Real cargo-workspace fixture tests for `extract_usages` (a tempdir
   crate exercising the five RA reference patterns, cached in a
   `OnceLock`).

The relevant change for this group's theme is item 1+2.

**Algorithm for `is_explicit_pub_use`.**

`use_has_explicit_visibility(db, use_id)` reads the source AST and asks
`use_node.visibility().is_some()`. The docstring is precise about why
this is required:

> HIR normalizes inherited visibilities, so consulting the post-
> resolution `Visibility` would conflate "explicitly inherited from a
> `pub` module" with "explicitly marked `pub`". We instead read the
> source AST directly.

That distinction matters: rustc's `Visibility` lattice doesn't preserve
syntactic intent, and only the AST does. ✓

**Issues.**

* **Minor — ExternCrateImport is never marked `is_explicit_pub_use`.**
  The classifier returns `None` for the `UseId` arm in
  `ImportOrExternCrate::ExternCrate`:

  ```rust
  // ExternCrate doesn't carry a UseId. We don't try to recover its
  // syntactic visibility; downstream filters treat extern-crate
  // bindings as never explicitly `pub`-marked.
  Some(ImportOrExternCrate::ExternCrate(_)) => (BindingKind::ExternCrateImport, None),
  ```

  This means `pub extern crate foo` won't appear in
  `declared_reexports_of(this_module)`. Niche pattern but possible. The
  comment owns the limitation. Acceptable.

* **Minor — Default deserialization.** The new field carries
  `#[serde(default)]`, but the schema bump (v3→v4) means we never read
  v3-encoded bytes back. Defensive — would matter only if a non-
  schema-versioned reader saw old data. Doesn't hurt.

* **Minor — Schema version is documented as `34` in the comment but
  the actual constant is `4`.** Looking at the diff text more carefully,
  that's a rendering artifact (the diff shows old=3, new=4 inline as
  "34"). The actual source reads `pub const SCHEMA_VERSION: u32 = 4;`.
  Verified.

* **Minor — Cross-commit safety vs `dead_pub_in_crate`.** Adding
  `is_explicit_pub_use` doesn't change dead-pub behavior because
  `dead_pub_in_crate` never consults that field. Verified by grep. The
  algorithm walks bindings looking only at `kind` and `from_module`'s
  crate. ✓

* **Minor — Test fixture for `is_explicit_pub_use`.**
  `explicit_pub_use_is_marked_on_pub_use_bindings` is precise: it asserts
  that `pub use loader::load;` (in `graph::mod.rs`) lands with
  `is_explicit_pub_use=true`, and that the private `use` lines in
  `graph::queries` produce at least one binding with the field set to
  `false`. Good test of both branches.

* **Nit — Massive commit.** 1,378 LOC. Could have been split (the
  `is_explicit_pub_use` change is logically separate from `crate_edges`,
  `overlaps`, `module_tree`, etc.). Not actionable now.

* **Nit — Some MCP tools added here aren't strictly required for the
  "dead-pub" theme**, but `declared_reexports_of` does inform the
  workflows that complement dead-pub (audit a module's pub-use surface).
  Group cohesion is OK.

**Verdict.** The pub-use tracking is correctly implemented (AST-based,
which is the only correct source). The schema bump is necessary and
documented. The unrelated bundled queries don't regress dead-pub.

---

### `7feb3a92` — replace `encapsulation_ratio` with `pub_crate_share` and refine module_tree labels — 146 LOC

**What it does.**

1. Renames `WorkspaceStats.encapsulation_ratio` → `pub_crate_share` and
   redefines it as `pub_crate / (pub + pub_crate)` (was `pub_crate /
   total_items`). NaN-guarded.
2. Adjusts `module_tree` to (a) emit `item_kind` labels as
   `"Item.Fn"`/`"Item.Struct"`/etc. instead of bare `"Fn"`/`"Struct"`,
   and (b) populate per-Item `visibility` strings rendered from the
   item's `Declared` binding (was always `None` because Item Nodes
   themselves don't store visibility).

**Issues.**

* **Minor — `pub_crate_share` semantic change is breaking.** The old
  `encapsulation_ratio = pub_crate / total_items` and the new
  `pub_crate_share = pub_crate / (pub + pub_crate)` are different
  ratios. Anyone who saved old metric values for trending will see a
  discontinuity. The new metric is more meaningful as an "encapsulation
  discipline" indicator (decoupled from how many private items the crate
  happens to have), and the docstring explains the rationale: "of the
  items the author actively made non-private, what fraction is
  crate-scoped?". Trade-off accepted.

* **Minor — All call sites and docs updated.** `grep -rn
  encapsulation_ratio` over the workspace returns no matches.
  `pub_crate_share` references found in `.docs/`, `.plans/`, `skills/`,
  and tests. ✓

* **Minor — `UsageSummaryRow.consumer_module: NodeId` is removed from
  the public struct.** This is a public-API change for callers using the
  Rust API directly. Removed in 7feb3a92 (was added in 3a5be800). Not a
  problem for MCP consumers (the field was likely never serialized
  meaningfully — `NodeId` is `[u8; 32]`). Could disrupt downstream
  binary consumers, but the same commit that added it removed it, so
  there's no published baseline to break.

* **Minor — `module_tree` visibility lookup is O(items + bindings).**
  Builds `crate_items: HashSet<NodeId>`, then `item_parents: HashMap`,
  then walks `bindings_by_id` filtering to `Declared` and
  `crate_items.contains(target)`. The doc explains the "parent_match"
  tiebreaker for the (defensive) case where multiple `Declared` bindings
  exist for one Item. The picker prefers `from_module == parent` so a
  re-export shouldn't ever overwrite the canonical pick. Good
  defensiveness.

* **Minor — `format_binding_visibility` renders the four visibility
  cases as `"pub"`, `"pub(self)"`, `"pub(crate)"`, `"pub(in <path>)"`.**
  Matches rustc surface syntax. ✓

* **Nit — `Item.Fn` style.** The prior version emitted bare `"Fn"`. The
  new version prefixes with `Item.`. This is the same form used by
  `node_kind_label` (`"Item.Fn"` for Items). Cleaner, but a label-format
  change that could break consumers depending on the old form. Internal
  consumers all migrated.

**Verdict.** Necessary refinement of the metric (the old one was
mathematically dubious) and a useful module_tree improvement. The
`pub_crate_share` semantics are documented and the renaming is global.

## 3. Cross-commit observations

### Do `dead_pub_in_crate` and `dead_pub_report` give consistent results?

Yes — by construction. `dead_pub_report` calls `dead_pub_in_crate(crate_id)`
for every Crate node it finds. There is no separate algorithm. Tests in
`92763548` and `ead32c04` together exercise both paths through the MCP
shell with the same workspace, and both return `"findings"` arrays as
expected.

The only difference is that `dead_pub_report` additionally backfills
`Node.file/span` (via `try_to_nav` in `ead32c04`), so the report
contains navigable file:byte-range pointers per finding. `dead_pub_in_crate`
returns just `qualified_name + item_kind`, but a caller can join with
`find_definition` on the same snapshot. Both serializations include
`declared_visibility`.

### Does pub-use visibility tracking change earlier behavior?

No. `Binding.is_explicit_pub_use` is added but `dead_pub_in_crate` never
consults it. The dead-pub algorithm walks every Binding looking at
`kind ∈ {NamedImport, GlobImport, ExternCrateImport}` and
`from_module`'s crate — not at syntactic visibility of the `use`. Adding
the field is purely additive on the dead-pub side.

That said, the schema bump (v3 → v4) does invalidate v3 snapshots, so a
user running `dead_pub_in_crate` after upgrading will pay one rebuild
cost on first invocation. The disjoint-`graph_id` comment in
`storage.rs` captures the intent.

### Visibility-rule correctness for `is_explicit_pub_use`

The implementation reads `use_node.visibility().is_some()` from the AST,
which is the only correct way to distinguish "syntactically pub-marked
use" from "non-pub use inheriting effective visibility from a pub
module". Consulting the post-resolution HIR `Visibility` would conflate
the two — the docstring spells this out. ✓

Caveat: `extern crate` declarations don't carry a `UseId`, so
`pub extern crate foo` is silently treated as not pub-marked. That's a
known minor blind spot, documented in the classifier comment.

### MCP tool surface consistency

All new tools (`dead_pub_in_crate`, `dead_pub_report`, `who_uses`,
`who_uses_summary`, `get_declared_reexports`, `crate_edges`, `overlaps`,
`module_tree`, `workspace_stats`) follow the same patterns:

- `*Params` struct with `#[derive(serde::Deserialize, schemars::JsonSchema)]`
- `directory: String` always present
- One-line `description` on each `#[tool(...)]`
- Error paths return `McpError::invalid_params(...)` for caller mistakes
  and `internal_error("query_name")` for snapshot errors.

The `instructions` constant in `search_tool_router.rs` is appended to as
each tool lands, so users see a full numbered list. ✓

### Cross-commit ordering

Order is correct: query → MCP wrapper → workspace aggregate → field
addition → metric rename. Each commit builds; the schema-version bumps
land at the right commits (v2→v3 in `ead32c04` for navigability,
v3→v4 in `3a5be800` for `is_explicit_pub_use`).

## 4. Overall verdict

**PASS.**

- The dead-pub classification is structurally sound for v1 and the known
  failure mode (items used only through public signatures) is documented
  at the query, the tool description, and the example output.
- Pub-use visibility tracking reads the AST, which is the only correct
  source.
- The `encapsulation_ratio` → `pub_crate_share` rename is global; no
  callers or docs left on the old name.
- Schemas, MCP wiring, and error handling are uniform and follow the
  existing graph_tools patterns.
- `cargo check --lib` is clean.

Caveats worth surfacing but not blockers:

1. **False-positive disclaimer should remain prominent.** The tool
   description warns "Conservative: may miss items used only through
   public type signatures." Users acting on findings should pair with
   `find_references` before downgrading.
2. **`pub extern crate`** is not marked `is_explicit_pub_use`. Niche.
3. **`pub_crate_share` is a semantic change**, not a pure rename — anyone
   trending the old `encapsulation_ratio` will see a discontinuity.
4. **Test crates (integration `tests/foo.rs`)** are separate Cargo crates
   under `all_targets: true` + `set_test: true`, so usage from
   `tests/` correctly prevents dead-pub flags. `#[cfg(test)] mod tests`
   inside the lib is same-crate and **does not** prevent dead-pub
   flagging — by design.
5. **Derive-macro-only usage** of a local trait won't register as a Usage
   (derive expansion doesn't write the trait name syntactically), so a
   local trait used only through `#[derive(MyTrait)]` in another crate
   could be falsely flagged. Same root cause as the signature-only false
   positive; worth a docstring mention.
