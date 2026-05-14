# Force 16 — Test seams & integration ergonomics

## Context

Tests in this project must coordinate Tantivy, LanceDB, ONNX (`fastembed`), `ra_ap_*`, sled, and heed. Cold integration costs ~115 s mostly because of (a) ONNX model load and (b) `ra_ap_load_cargo` cache prefill. Project memory already warns: "Avoid `cargo test` here; use `cargo check --lib`." Layout choice must reduce that pain without driving the suite into mock-only theatre.

## Status quo: single crate

- `pub(crate)` everything; tests live under `tests/` and `src/.../mod.rs`.
- "Search returns results after indexing": straightforward — call `UnifiedIndexer::index_directory_parallel` against a `tempfile::TempDir`, then `HybridSearch::search`. Already what the few real integration tests do.
- Embedding mock: no seam. `EmbeddingGenerator` is a concrete struct around `Arc<Mutex<TextEmbedding>>`. Tests load real ONNX once per process; under `cargo test` that means once per test binary, repeated for every integration target → snapshot-build pain.
- LanceDB lock contention: every test opens its own `TempDir`, but parallel tests inside one binary that share an env can collide on the file lock. Today mitigated by `--test-threads=1`, which compounds the 115 s figure.
- Encourages real tests, but the cost forces developers to skip them.

## Five-crate capability split

- `core`, `index`, `search`, `graph`, `semantic`, `server`. Each ships its own `tests/`.
- "Search after indexing" must cross `index` and `search` crates → either a thin `test-support` crate that owns the `TempDir` + fixture corpus, or the test moves up to `server` (slowest binary).
- Embedding mock: `index` and `search` depend on `embeddings` crate, so a `cfg(feature = "test-fakes")` dummy generator can be exposed once and reused. Cleaner than today.
- LanceDB lock: capability crates can keep their integration tests narrow (each gets its own tempdir, no cross-test sharing). Fewer accidental collisions because suites are smaller.
- Encourages real tests at the *crate* level; cross-capability flows still need server-level tests, which remain slow.

## Hexagonal split with traits

- Every backend behind a trait: `EmbeddingBackend`, `VectorIndex`, `KeywordIndex`, `SemanticHost`, `GraphSnapshot`, `MetadataStore`.
- "Search after indexing" with fakes: trivial, sub-second. With real backends: same as today plus an extra layer of dyn dispatch.
- Embedding mock: native — implement `EmbeddingBackend` with deterministic hashed vectors. Solves ONNX-load-per-test entirely.
- LanceDB lock: a `MemoryVectorIndex` fake removes contention from 90% of tests; the remaining real-LanceDB tests run serially in one dedicated suite.
- Risk: layout *encourages mock-only tests*. Real coverage shrinks unless contract tests (one trait-test module exercised against every impl, real and fake) are mandated.

## Pipeline-keyed split

- Crates per stage: `walker`, `parser`, `chunker`, `embedder`, `tantivy_writer`, `lancedb_writer`, `query`. Stages communicate via typed records (`ParsedFile`, `Chunked`, `Embedded`).
- "Search after indexing": each stage tested by feeding canned inputs and asserting outputs. End-to-end test still requires composing the whole pipeline → lives in `server` or a top-level `e2e` crate; same cost as status quo.
- Embedding mock: each stage takes its predecessor's record; you skip the embedder by feeding `Embedded` rows directly. No trait needed.
- LanceDB lock: writer stage owns the lock; isolated and easy to serialize.
- Encourages real-but-narrow tests per stage, but cross-stage regressions (the bugs that actually bite) still need slow e2e runs.

## Recommendation

**Hexagonal split with mandatory contract tests.** It is the only layout that decisively kills the ONNX-per-test and LanceDB-lock costs while still allowing real coverage:

1. Define `EmbeddingBackend`, `VectorIndex`, `KeywordIndex`, `SemanticHost`, `GraphStore` as traits in a `ports` crate.
2. Provide `mem-fakes` (deterministic embeddings, in-memory vector index, RAM Tantivy via `RamDirectory`, no-op semantic host) — used by 95% of tests; sub-second.
3. A single `tests/contracts/` suite runs the same trait-level test matrix against both fake and real implementations. Real-backend run is gated behind `--features real-backends` and serialized; it is the *only* place ONNX loads, so the 115 s cost is paid once per CI run, not once per test binary.
4. Cross-capability and pipeline tests can layer on top — they inherit fast fakes by default and opt into real backends explicitly.

This pairs with the five-crate capability split (recommendation in force 15) by making the capability boundary the trait boundary; pipeline-keyed and status-quo layouts cannot match the speedup without giving up real coverage.
