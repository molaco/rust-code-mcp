# Parser Placement

## Decision

**Delete the standalone `parser` crate. Do not create `code-search::parse`. Do not duplicate `imports.rs` / `call_graph.rs` / `type_references.rs` into a graph crate.**

The `ra_ap_syntax`-based parser is a legacy artifact from a pre-rust-analyzer era. The `graph` module already loads the workspace through `ra_ap_load_cargo` into a `RootDatabase` + `Vfs`, runs HIR-grade extraction, and persists everything to LMDB. Anything `parser` produces is a strictly weaker projection of what `graph::extract` already computes:

| `parser` output | Already in snapshot |
| --- | --- |
| `Symbol` (Fn/Struct/Enum/Trait/Impl/Module/Const/Static/TypeAlias) | `Node` + `ItemKind` via `bindings` + `impls` |
| `CallGraph` (textual last-segment names) | `Usage` rows with `UsageCategory::Call` + `ast_resolve` for turbofish-safe HIR resolution |
| `Import` (use-tree flattening) | `Binding { kind: Import, ... }` from `DefMap` walk |
| `TypeReference` + `TypeUsageContext` | `Usage` rows tagged by category, with HIR-precise type info in `signatures` / `statics` |
| `extract_visibility` (textual `pub(crate)`) | `BindingVisibility` resolved against the def-map |
| `extract_docstring` | `attributes::extract_attributes` (HIR doc comments + outer attrs) |

The HIR variants are name-resolved, type-aware, and edition-correct. The `ra_ap_syntax` variants are textual heuristics. Keeping both is not "accepting duplication" — it is shipping two answers to the same question, where one is wrong more often.

## The rule

> Every Rust structural fact in the workspace is sourced from a single `OpenedSnapshot`. There is no second AST parser. If a consumer needs structure, it queries the snapshot. If the snapshot does not yet expose the projection it needs, the snapshot grows a query — not a parallel parser.

Concrete crate ownership in the new workspace:

- `code-graph` (the renamed `graph` crate) owns `loader`, `extract`, `model`, `snapshot`, `queries`, `ast_resolve`, and all audits. It is the **only** crate that depends on `ra_ap_*`.
- `code-search` (ingest) depends on `code-graph` for structure. The chunker becomes a `code-graph` consumer:
  - "symbols for file" → `OpenedSnapshot::nodes_in_file(path)`
  - "outgoing calls for symbol" → `OpenedSnapshot::calls_from(node_id)`
  - "imports for file" → `OpenedSnapshot::imports_of(module_id)`
  - "module path for file" → `OpenedSnapshot::module_tree` walk (replaces `extract_module_path`'s `src/` heuristic).
- `code-search` keeps `chunker`, embedding, Tantivy, LanceDB, Merkle. These are genuinely chunking/IR/storage concerns and don't belong in `code-graph`.

`imports.rs` / `call_graph.rs` / `type_references.rs` are **not** split across owners — they are deleted. Their replacements already live in `bindings.rs`, `usages.rs`, and `ast_resolve.rs` inside `code-graph`.

## Why not the alternatives

- **Keep `parser` as a shared crate, used by both.** This is the dumping-ground risk made real: the crate already mixes symbol extraction, call graphs, import flattening, and type-reference walking, all behind one `RustParser`. A shared parser crate also forces `code-search` to pull `ra_ap_syntax` as a transitive dep alongside the `ra_ap_hir` chain that `code-graph` already brings — two parsers, one workspace.
- **Duplicate into `code-search::parse` and `code-graph::extract::ast`.** A "contract" between two AST walkers does not survive contact with edition changes, new syntax (e.g., `let-else`, `async fn` in traits), or visibility rule tweaks. Bug fixes propagate by ritual, not by compiler. The HIR pipeline already handles all of this for free via rust-analyzer.
- **Per-symbol HIR is too slow for chunking.** Snapshots are already content-fingerprinted and reused across runs (`build_and_persist` short-circuit). The chunker's per-file cost becomes an LMDB read, which is cheaper than re-parsing with `ra_ap_syntax`.

## Top 3 risks

1. **Chunker latency on cold snapshot.** The first run has to build the HIR snapshot before any chunking starts, where today `parser` is just `fs::read_to_string` + `SourceFile::parse`. Mitigation: `index_codebase` already triggers `build_hypergraph`; gate ingest on snapshot-ready and reuse the fingerprint cache. Cold-build cost is one-time per workspace fingerprint.
2. **HIR coverage gaps for embedding context.** `format_for_embedding` wants cheap textual data (last-segment call names, raw `use` paths) that the snapshot currently exposes only via `Usage` + `Binding` rows. Mitigation: add thin `queries::*_for_chunking` accessors on `OpenedSnapshot` that return exactly the strings the chunker needs; do not let the chunker reach into `ra_ap_hir`.
3. **`code-graph` becoming the new dumping ground.** With `parser` gone, every "I need to know X about Rust code" request will land here. Mitigation: enforce a layering rule — `extract/` writes the model, `queries/` reads the snapshot, `audits/` compose queries. New consumers add a query method, not a new extraction phase, unless the model genuinely lacks the data.
