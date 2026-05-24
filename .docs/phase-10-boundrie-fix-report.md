# Phase 10 Boundrie Fix Report

## Scope

Phase 10 tightened the `rmc-engine` public surface while keeping it the
lowest-level primitive crate. Production consumers were moved to one-level
facade exports, implementation modules were made private where active
consumers allowed it, and embedding profile/backend ownership was documented
without changing embedding behavior.

## Steps Completed

1. Ran `jj show --summary`.
2. Confirmed active consumers of engine implementation modules.
3. Migrated production BM25 consumers to `rmc_engine::search::Bm25Search`.
4. Made search, vector-store, and parser implementation modules private while
   preserving facade reexports.
5. Documented `EmbeddingProfile` as the engine-owned embedding configuration
   model.
6. Confirmed embedding backend semantics were not changed in this phase.
7. Documented `EmbeddingBackend` as the formal cross-crate embedding runtime
   boundary.
8. Ran focused nix checks.
9. Recorded the Phase 10 ledger.

## Evidence

- MCP `who_imports` found only engine test-module glob imports for
  `search::bm25`, `search::resilient`, `search::rrf_tuner`,
  `vector_store::lancedb`, and `vector_store::traits`.
- Source search found production inline
  `rmc_engine::search::bm25::Bm25Search` references in indexing/server code;
  those callers now use `rmc_engine::search::Bm25Search`.
- Parser helper-module search found no external importers for `imports` or
  `call_graph`; `type_references` is only used inside `rmc_engine::parser`.
- `rmc_engine::search`, `rmc_engine::vector_store`, and
  `rmc_engine::parser` implementation modules are now private, with existing
  public facade reexports preserved.
- `EmbeddingProfile` remains in `rmc_engine` and is documented as the
  engine-owned profile schema and built-in registry model.
- `EmbeddingBackend` is documented as the shared runtime, cache identity, and
  dimension contract used by indexing, graph, and server crates.

## Files Changed

- `crates/rmc-engine/src/search/mod.rs`
- `crates/rmc-engine/src/vector_store/mod.rs`
- `crates/rmc-engine/src/parser/mod.rs`
- `crates/rmc-engine/src/embeddings/backend.rs`
- `crates/rmc-engine/src/embeddings/profile.rs`
- `crates/rmc-indexing/src/indexing/tantivy_adapter.rs`
- `crates/rmc-indexing/src/indexing/unified.rs`
- `crates/rmc-server/src/tools/endpoints/query.rs`
- `.docs/boundries-cleanup-progress.md`
- `.docs/phase-10-boundrie-fix-report.md`
- `.plans/boundries-plan.md`

## Verification

- Focused indexing/server check passed:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-indexing -p rmc-server`.
- Focused engine/dependent-crate check passed:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-engine -p rmc-indexing -p rmc-server -p rust-code-mcp`.
- Final focused check passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-engine -p rmc-indexing -p rmc-server -p rust-code-mcp`.
- No formatting command was run.

## Commits

- `0a605e3f`: `docs: start phase 10 engine surface`
- `55416fc3`: `docs: record phase 10 engine consumers`
- `a40c2790`: `refactor: use engine search facade types`
- `187b466c`: `refactor: tighten engine implementation modules`
- `8aab7525`: `docs: document embedding profile ownership`
- `b0b7b27e`: `docs: confirm embedding backend semantics`
- `27ebe7af`: `docs: document embedding backend boundary`
- `93934632`: `docs: record phase 10 check result`
- `91187faa`: `docs: record phase 10 ledger`

## Outcome

Phase 10 success criteria are met. `rmc-engine` remains the lowest-level
primitive crate, production consumers prefer one-level facades, and embedding
profile/backend types are documented as deliberate engine-owned boundary
types.
