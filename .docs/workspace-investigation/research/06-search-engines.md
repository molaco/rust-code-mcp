# Workspace Layouts of Production Rust Search Engines

Investigation of how Tantivy, Meilisearch, and Qdrant split their Cargo workspaces, with lessons for an MCP code-search server.

## Tantivy (quickwit-oss/tantivy)

Tantivy is a *library*, not a server. The repo root **is** the public crate (`tantivy` v0.26) and the workspace re-uses sub-crates as path dependencies.

Workspace members (8 internal crates + root):
- `query-grammar` (`tantivy-query-grammar`) - parser for the query DSL.
- `bitpacker` (`tantivy-bitpacker`), `columnar` (`tantivy-columnar`), `sstable` (`tantivy-sstable`), `stacker` (`tantivy-stacker`) - storage / encoding primitives.
- `common` (`tantivy-common`), `ownedbytes`, `tokenizer-api` (`tantivy-tokenizer-api`) - shared utilities and stable extension points.

**Public API:** single facade. Users depend on `tantivy` only; sub-crates are renamed (`tantivy-*`) and re-exported. The root crate's `[features]` (`mmap`, `stemmer`, `lz4-compression`, `zstd-compression`, `quickwit`) toggle which sub-crates compile in.

**Storage vs query:** clean separation by sub-crate. Encoding/IO (`columnar`, `sstable`, `bitpacker`, `stacker`, `ownedbytes`) lives outside the root; query parsing is fully isolated in `query-grammar`. The root crate stitches them into Index, Searcher, Collector.

**Cross-cutting types:** `tantivy-common` holds shared types/IO traits; `tokenizer-api` is a deliberately tiny stable ABI so external tokenizer crates can plug in without depending on the full engine.

## Meilisearch (meilisearch/meilisearch)

A binary product. ~22 internal crates plus a vendored `external-crates/` (forked `async-openai`).

Headline crates:
- `meilisearch` - the HTTP server binary.
- `milli` - the actual search engine (heed/LMDB, charabia tokenizer, ranking, geo, vectors).
- `meilisearch-types`, `meilisearch-auth` - shared DTOs, auth.
- `index-scheduler`, `dump`, `file-store` - durability/orchestration layer.
- `filter-parser`, `flatten-serde-json`, `json-depth-checker`, `permissive-json-pointer` - small focused parsers/utilities.
- `routes`, `routes-macros`, `http-client`, `openapi-generator` - HTTP surface.
- `meilitool`, `xtask`, `benchmarks`, `fuzzers`, `tracing-trace`, `meili-snap`, `build-info` - tooling/dev.

**Public API:** none. `milli` is `publish = false`; the only consumer is the server. The workspace is a closed graph of internal crates.

**Storage vs query:** `milli` owns both LMDB indexes and ranking/search; durability and queueing live one layer up in `index-scheduler` + `file-store` + `dump`. HTTP routing/auth sits above that.

**Cross-cutting types:** `meilisearch-types` is the shared DTO crate; `[workspace.package]` centralises version/license; `[workspace.dependencies]` is intentionally thin (just mimalloc).

## Qdrant (qdrant/qdrant)

A binary product with a much heavier shared-types footprint. Root crate `qdrant` is the server; ~16 library crates under `lib/`.

Headline crates:
- `lib/segment` - on-disk vector segment, indexes, payload.
- `lib/collection` - collection abstraction over segments, search/scroll, replication.
- `lib/storage` - top-level storage manager, persistence.
- `lib/shard` - sharding/clustering primitives.
- `lib/api` - gRPC/REST DTOs and trait surface.
- `lib/wal`, `lib/gridstore`, `lib/posting_list`, `lib/sparse`, `lib/trififo` - storage primitives.
- `lib/common/*` - flat namespace of `common`, `cancel`, `issues`, `dataset` shared across everything.
- `lib/macros`, `lib/gpu`, `lib/edge`, `lib/edge/python` - macros, GPU kernels, FFI.

**Public API:** none externally; binary-only. `api` crate is the cross-internal contract.

**Storage vs query:** strict layered cake - `wal`/`gridstore`/`posting_list` -> `segment` -> `collection` -> `shard` -> `storage` -> `qdrant` (HTTP/gRPC). `api` types cross all layers without pulling implementation deps.

**Cross-cutting types:** aggressive `[workspace.dependencies]` (~80 entries) plus `[workspace.lints]` (clippy/rustdoc). Errors are per-crate `thiserror`. Cancellation lives in its own `cancel` crate so it can be shared without dragging in `tokio` everywhere.

## Common patterns

1. Tiny shared-types crate (`tantivy-common`, `meilisearch-types`, `lib/api` + `lib/common/*`) - DTOs and traits used everywhere, with minimal deps.
2. Storage primitives extracted into leaf crates so they can be reused, fuzzed, and benchmarked in isolation.
3. Parsers/grammars isolated (`query-grammar`, `filter-parser`) - small, no I/O, snapshot-testable.
4. `[workspace.package]` + `[workspace.dependencies]` for version pinning and metadata reuse (Meili, Qdrant).
5. Tooling crates (`xtask`, `meilitool`, `fuzzers`, `benchmarks`) live in the workspace, not in `dev-dependencies`.
6. Feature flags propagate from the top crate down to sub-crates (`tantivy`'s `mmap`, Qdrant's `tracing`/`gpu`/`rocksdb`).

## Differences

- **API surface:** Tantivy = single facade re-exporting renamed sub-crates; Meili/Qdrant = closed internal graph, no public crate.
- **Granularity:** Tantivy ~9 crates, Meili ~22, Qdrant ~16 + nested `common/*`. Library projects stay smaller; servers fan out.
- **Errors:** all use `thiserror` per-crate; none ship a global error crate.
- **Workspace lints:** Qdrant centralises clippy/rustdoc lints; Tantivy and Meili don't.

## Direct lessons for our project

- Adopt Qdrant's layered split: `*-types` (DTOs/traits) -> storage primitives -> indexer -> query/graph -> server. Keep `*-types` dep-light so every layer can import it.
- Put parsers (Rust query/filter syntax, path globs) in their own no-I/O crates - matches `filter-parser` / `query-grammar` and pays off in test speed.
- Centralise versions via `[workspace.package]` and `[workspace.dependencies]`; add `[workspace.lints]` from day one (Qdrant pattern).
- Keep `xtask`, fuzzers, and bench harnesses as workspace members, not hidden in dev-deps.
- Don't build a "single facade" crate (Tantivy-style) unless we plan to publish a library. As an MCP server we are Meili/Qdrant-shaped: closed graph, internal `*-types` contract, binary at the root.
- Search-engine-specific and **skip**: WAL/segment/sharding crates, tokenizer ABI crate, columnar/sstable storage primitives, vector/GPU kernels. Our "storage" is the persisted hypergraph plus snapshot files - one focused crate, not a stack.
- Cancellation-as-its-own-crate (Qdrant `cancel`) is worth copying: MCP request cancellation cleanly decouples from tokio specifics.
