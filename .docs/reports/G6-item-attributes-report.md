# G6 — Item attribute extraction (commit 75ee85c9)

## 1. Feature summary

This commit adds a syntax-level item-attribute extraction pass and exposes two MCP query tools (`item_attributes`, `items_with_attribute`) plus a re-export-chain / pub-type-alias / forbidden-dependency / crate-dependency-metric / enum-variants bundle that all share the same wiring style. The attribute pass walks every local crate via `Semantics`, iterates `Module::declarations` / `Trait::items` / `Impl::all_in_crate(...).items` / `Enum::variants`, and on each definition resolves its AST source via `HasSource::source(db)`, then captures the trimmed text of every outer `#[...]` attribute and every `///` / `/** */` doc-comment line into a new `Node.attributes: Vec<String>` field. Storage version is bumped from v7 to v8; the `Node` bincode record gains one `#[serde(default)]` field and old snapshots auto-rebuild because `graph_id_for` mixes `SCHEMA_VERSION`.

The 2.2k LOC tally is somewhat misleading — only ~350 lines are the attribute pass itself (`src/graph/attributes.rs`), and ~150 lines are tests/example timing. The bulk of the diff is in `queries.rs` (873 added lines), but most of that diff is the four sibling features piggy-backed in the same commit: `forbidden_dependency_check`, `pub_use_pub_type_audit`, `re_export_chain`, `crate_dependency_metric`. The review below covers the attribute feature in depth and flags the bundling as a process concern.

## 2. Subsystem review

### 2.1 Model & schema

**Files**: `src/graph/model.rs` (+17 LOC), `src/graph/storage.rs` (+22 LOC)

What changed:
- `Node` gains `pub attributes: Vec<String>` with `#[serde(default)]`.
- `SCHEMA_VERSION` bumped to v8 with a thorough explanatory comment (`storage.rs:74–86`).
- All Node constructors across `extract.rs`, `bindings.rs`, `impls.rs` are updated to initialize `attributes: Vec::new()`.

Findings:
- **OK**: schema-version bump is correct — bincode would reject v7 records, and the `graph_id_for` hash includes `SCHEMA_VERSION` so old graph_ids never collide. Stale snapshots are detected via `read_manifest_compatible` returning `Ok(None)`.
- **MINOR**: doc-comment on `Node.attributes` (`model.rs:108–119`) is good — explains "one entry per line", "inner attrs not collected", "empty for synthetic items". The wire format (raw trimmed text) is documented at the call site too.
- **MINOR**: `Vec<String>` storage is unkeyed — substring queries are the only retrieval pattern. There is no structured representation distinguishing `#[derive(...)]` from `#[cfg(...)]` from `///` doc lines. That is by design (see `attributes.rs:9–15`) but it means callers wanting "items that derive `Clone`" have to substring-match `"Clone"` and risk hitting `#[derive(MyClone)]` or a `/// Clone-related ...` doc comment. **Acceptable tradeoff; flag in user-facing docs.**

Verdict: **PASS**.

### 2.2 Extraction pass

**File**: `src/graph/attributes.rs` (new, 350 LOC), `src/graph/extract.rs` (+22 LOC scheduling)

What changed:
- New `extract_attributes` entry point invoked between `extract_impl_items` and `extract_usages` (correct ordering — needs the v5 methods / assoc consts / assoc types and v7 enum-variant Items to exist in `def_to_node` first).
- Walks `Module::declarations` recursively (only within the same crate), `Trait::items`, inherent impl `items()`, and `Enum::variants`.
- For each definition, looks up its `ModuleDefId` in `def_to_node`, resolves the AST source via `HasSource::source(db)`, then iterates `attrs()` (outer only, via `attr.kind().is_outer()` filter) and `doc_comments()`.
- Stores raw `attr.syntax().text().to_string().trim()` per attribute; doc comments are normalized to `"/// <body>"` with multi-line `/** */` blocks split per line.

Coverage findings:

| Attribute kind | Captured | Notes |
|---|---|---|
| `#[derive(...)]` | Yes | Raw text — derive list is one string, not split into individual traits |
| `#[cfg(...)]` / `#[cfg_attr(...)]` | Yes | Captured as raw source text; not evaluated for cfg-active-ness |
| `#[must_use]`, `#[non_exhaustive]`, `#[inline]`, `#[deprecated]` | Yes | All bare and parameterized forms |
| Custom proc-macro attrs (`#[tokio::main]`, `#[serde(rename = "...")]`) | Yes | Source-text preserved verbatim |
| Outer doc comments (`///`) | Yes | One entry per line |
| `/** ... */` block doc comments | Yes | Split per source line |
| Inner attributes (`#![allow(...)]`) | **No** | Explicitly filtered via `is_outer()`; this matches the doc-comment design intent but the field comment on `Node.attributes` claims this and it's enforced |
| **Module-level attributes** | **No** | `visit_module` does NOT call `set_attrs_for` on the module itself — `#[deprecated] mod foo;` is invisible. Same for crate-root attrs. |
| Macro-expansion-only items | No | `source(db)` returns `None`, silently skipped |

Issues found:

- **MINOR — Module attributes not extracted.** The pass walks `module.declarations(db)` for items inside modules but never records attributes on Module Nodes. So `#[deprecated] mod foo;` and feature-gated submodules (`#[cfg(feature = "x")] mod x;`) won't surface via `items_with_attribute`. This is consistent with the "items only" framing in the file header but is a documented gap rather than a closed feature. The Node struct already stores `Vec<String>` for any NodeKind, so the field would just stay empty on Modules. **Not blocking, but worth a follow-up.**

- **MINOR — Multi-line `#[derive(...)]` retains embedded newlines.** Source like
  ```rust
  #[derive(
      Debug,
      Clone,
  )]
  ```
  is captured as one entry with internal `\n` characters. Substring searches still work (`"Debug"` matches), but the string is bulkier and human-readable rendering is awkward. No re-flow / whitespace-collapse is applied. **Acceptable for v8 but consider normalizing.**

- **MINOR — Defensive idempotency check on re-population is too coarse.** `set_attrs_for` returns early if `item_node.attributes.is_empty()` is false (line `attributes.rs:234`). This means re-visiting an item through a re-export path won't overwrite — correct. But it also means that an item that genuinely has no attributes will get re-processed on every visit and produce an identical empty Vec. The early return on `is_empty()` is correct (the more common case is "no attrs") but the comment says "defensive against double-visits via re-exports" which doesn't quite match: re-exports route through `def_to_node` lookups, and the same `def_id` produces the same node, so double-visit risk is from `Trait::items` + `Impl::all_in_crate(...).items` visiting overlapping defs. Worth verifying that's the real motivation.

- **MINOR — `Impl::all_in_crate` is NOT used.** The header comment in `attributes.rs:20–22` says the pass mirrors `impls.rs` shape and walks "Impl::all_in_crate(...).items" — but the actual code walks `module.impl_defs(db)` (line 122). Both should be equivalent in coverage for local crates, but the doc-comment is misleading.

- **OK — Trait-impl bodies are intentionally skipped** (`attributes.rs:124–127`, mirroring `impls.rs`'s policy of only emitting inherent-impl assoc items). Trait-impl methods don't have Item nodes, so the early-return makes sense.

- **PERF (minor) — Hot path overhead.** The pass adds full re-traversal of every module's declarations and impls, with `source(db)` calls per item. `source` is non-trivial in RA (it parses the file's syntax tree if not already cached). For a workspace with ~10K items this is bounded; the `EXTRACT_TIMING` plumbing added in this commit (line 132–151 of extract.rs) is exactly the right tool for measuring. **No regression test for performance but the timing infrastructure is in place.**

Verdict: **PASS with MINOR caveats**. The pass is straightforward, well-documented, and uses the right RA APIs. Gaps (module-level attrs, derive normalization) are documented and could be follow-ups.

### 2.3 Storage / LMDB

**File**: `src/graph/storage.rs`

What changed:
- `SCHEMA_VERSION = 11` (this commit bumped from v7 to v8; subsequent commits added v9/v10/v11).
- No new sub-DB for attributes — they live inside the bincode-serialized `Node` record under `nodes_by_id`.
- Storing inside the Node record means `item_attributes(target)` is a single key-lookup (already loading the full Node).
- `items_with_attribute(crate_id, pattern)` does a full table scan over `nodes_by_id`, filtering by `crate_id` and substring-matching every attribute.

Findings:
- **OK — Backward compatibility**: `#[serde(default)]` on the new field would let bincode tolerate missing fields under serde_json, but bincode is positional and rejects extra/missing fields at the binary layout. So in practice old v7 snapshots won't deserialize — and the schema bump triggers a rebuild before any deserialize is attempted. The comment in storage.rs explicitly acknowledges this.
- **OK — No migration script needed**: the design philosophy is "rebuild on version bump". `build_and_persist` lands at a different `graph_id` directory; old snapshots remain on disk for manual cleanup. Documented in commit-history-style comments at the top of `storage.rs`.
- **MINOR — No secondary index for attribute search**. Every `items_with_attribute` call is O(N items × M attrs) substring matches over the whole `nodes_by_id` table. For a workspace audit tool this is fine, but a precomputed inverted index (attr-text → set of NodeIds) would let `#[must_use]` audits run in O(1) lookup. **Not a v8 requirement.**
- **OK — `embeddings_by_target` and other lazy caches** in the same env aren't touched.

Verdict: **PASS**.

### 2.4 Tool surface (MCP)

**Files**: `src/tools/search_tool.rs` (+72 LOC), `src/tools/search_tool_router.rs` (+56 LOC), `src/tools/graph_tools.rs` (+378 LOC)

What changed:
- Two new tools wired through the standard `#[tool]` macro path:
  - `item_attributes(directory, target)` → returns `{target, item_kind, file, span, attribute_count, attributes}`
  - `items_with_attribute(directory, crate_name, attribute_pattern)` → returns `{crate, attribute_pattern, match_count, items: [{qualified_name, item_kind, matched_attribute, file, span}]}`
- Plus the four bundled tools (`forbidden_dependency_check`, `pub_use_pub_type_audit`, `re_export_chain`, `crate_dependency_metric`, `enum_variants`) — those are out-of-scope for this review but share the same wiring.

Findings:
- **OK — Schemas**: each param struct uses `#[derive(serde::Deserialize, schemars::JsonSchema)]` with `#[schemars(description = "...")]` on each field. Descriptions are concrete with examples (`"my_crate::Foo"`, `"#[must_use]"`, `"derive(Debug"`).
- **OK — Error paths**: lookup failures produce `McpError::invalid_params` with the user-provided qualified name in the message. `internal_error` wraps anyhow errors for the heed/RA layer.
- **OK — Crate-name resolution**: `items_with_attribute` accepts either a Crate or a root Module qualified name (`graph_tools.rs:472–492`), with a clear error if a non-crate/non-module is passed. Mirrors the `pub_use_pub_type_audit` resolution.
- **MINOR — Pattern semantics are substring, not anchored**. The tool description says "substring match"; the implementation calls `a.contains(attr_pattern)`. So `items_with_attribute(crate, "derive")` will find both `#[derive(Debug)]` AND `/// derive a value here`. The schema description does include examples like `"#[must_use]"` and `"derive(Debug"` to guide callers to use bracket-anchored patterns, but a naive caller passing `"derive"` will get false positives from doc comments. **Worth tightening — note: subsequent commits added an "anchored attribute match" mode, addressing this.**
- **MINOR — `matched_attribute` only returns the FIRST match.** If an item has both `#[derive(Debug)]` and `#[derive(Clone)]` (two separate derive attrs) and the pattern matches both, only the first is reported. Callers wanting full coverage would need to call `item_attributes(target)` per row.
- **OK — Output shape**: `Enriched*` response types use `#[serde(skip_serializing_if = "Option::is_none")]` consistently, so empty file/span don't pollute JSON.
- **OK — `item_kind_label` covers the new EnumVariant variant** added in v7 (`graph_tools.rs:853`).

Verdict: **PASS**.

### 2.5 Tests

**File**: tests at the bottom of `src/graph/attributes.rs` (3 tests) and `src/graph/queries.rs` (2 tests).

Coverage:
- `attributes_of_known_struct` — verifies `Node` struct's derive list contains Debug, Clone, PartialEq, Eq, Serialize, Deserialize.
- `attributes_of_known_enum` — same for `ItemKind` enum.
- `attributes_of_item_with_no_attrs_is_empty` — `set_attrs_for` itself has no non-doc attrs.
- `item_attributes_of_node_struct_includes_derive` — duplicates the attributes test from a query-side perspective.
- `items_with_attribute_finds_derive_users` — passes `"derive"` substring across the whole `file_search_mcp` crate, asserts Node and ItemKind both appear and that every hit's `matched_attribute` contains "derive".

Findings:
- **MINOR — No negative tests** — no test for "this item that does NOT have a `#[must_use]` is correctly excluded". The `attributes_of_item_with_no_attrs_is_empty` test is the closest but it's positive on absence rather than negative on exclusion.
- **MINOR — No cfg / non-derive coverage**: tests only target `#[derive(...)]` and doc-comments. No test for `#[must_use]`, `#[non_exhaustive]`, `#[inline]`, `#[cfg(...)]`, custom attrs, deprecated, or attribute proc macros. For a 350-LOC feature claiming broad attribute coverage, this is light.
- **MINOR — No EnumVariant attribute test**: the pass explicitly walks `enum.variants()` (`attributes.rs:166–177`) and stores per-variant attributes, but no test verifies a variant-level `#[non_exhaustive]` or doc comment surfaces correctly.
- **MINOR — No assoc-item attribute test**: same for inherent-impl methods and trait-declaration methods.
- **MINOR — Substring-match false-positive case isn't tested.** Callers could pass `"derive"` and hit `/// Derived from Foo` doc comments. Test would catch this regression if the implementation tightened up.
- **OK — Shared snapshot infrastructure is reused** via `queries::tests::shared_snapshot()`, so tests don't redundantly rebuild.

Verdict: **MINOR — proportionate but light.** A 350-LOC extraction pass with five integration-style tests covers the happy path but doesn't exercise the breadth of attribute kinds claimed in the docstring. Subsequent commits do tighten this (the `items_with_attribute_does_not_match_pattern_inside_attr_body` test now in the working tree didn't exist at 75ee85c9).

### 2.6 Process / commit hygiene

The commit message is `[review] add item attribute extraction to graph snapshot` — but the diff also lands `forbidden_dependency_check`, `pub_use_pub_type_audit`, `re_export_chain`, `crate_dependency_metric`, `enum_variants`, plus the `examples/timing_extract.rs` profiling binary and `EXTRACT_TIMING` instrumentation across `extract.rs` / `snapshot.rs`. The 873-line `queries.rs` diff is mostly these other features, not the attribute pass.

**MINOR — Bundling concern.** Each of those features would have been independently reviewable; bundling them under "add item attribute extraction" makes bisect and review noisy. The `[review]` tag in the subject suggests this was a review-batch commit deliberately, but flagging for future-proofing.

## 3. Overall verdict

**PASS — MINOR**.

The attribute extraction is a clean, well-documented syntax-level pass with the right RA API choices (`HasAttrs::attrs()`, `HasDocComments::doc_comments()`, outer-only filtering). Schema-version bump is correct, backward-compat path is the established rebuild-on-bump pattern, and the two MCP tools have validated schemas and clear error paths. The known gaps (no module-level attrs, multi-line derive normalization, substring-vs-anchored pattern semantics, sparse test coverage of non-derive attribute kinds) are all "MINOR" — none break the contract or risk data corruption. Subsequent commits in the working tree have already addressed at least the anchored-match concern, suggesting the team is iterating on the rough edges.

Recommended follow-ups (none blocking):
1. Capture module-level attributes (cheap; the visit loop already has the Module handle).
2. Normalize whitespace inside multi-line `#[derive(...)]` so storage is single-line.
3. Add `#[must_use]` / `#[non_exhaustive]` / `#[cfg(...)]` / `EnumVariant` / assoc-item attribute tests.
4. Split future bundled commits — one feature per commit per the existing pattern of v5/v6/v7/v9/v10/v11.
