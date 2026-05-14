# Force 17 — Public API Surface & SDK Consumability

## Constraint

Today the project ships one binary (`rust-code-mcp-final`) plus an implicit `lib.rs` that re-exports every subsystem. There is no external Rust consumer. A workspace split is the natural moment to design for one — embedding `code-search` into a CI tool, or `graph` into a doc generator — without committing to it now.

The hard rule: **no SDK types in public APIs**. `tantivy::Index`, `lancedb::Connection`, `ra_ap_hir::Semantics`, `heed::Env`, `fastembed::TextEmbedding` must not appear in any `pub fn` signature a downstream crate would call. They leak transitive dependency graphs (CUDA, ICU, ONNX, LLVM-via-RA) and lock the consumer into our exact versions.

## Candidate Layouts

### 1. Single crate (status quo)

**SDK experience.** A consumer adds `rust-code-mcp-final = "x.y"` to `Cargo.toml` and pulls **everything**: rmcp, tokio multi-thread runtime, fastembed/ort, tantivy, lancedb, heed, every `ra_ap_*` crate, sled, sysinfo, directories. Roughly 600 transitive deps for a tool that wanted only `who_calls`. Compile times alone (~3–5 min cold) make this a non-starter.

**Type leakage.** `lib.rs` declares modules with `pub mod`, so `RustParser`, `OpenedSnapshot`, `HybridSearch`, `EmbeddingGenerator` are reachable — but their methods take and return `tantivy::*`, `ra_ap_*`, `heed::*` directly. There is no boundary at which to enforce "no SDK types". `#![warn(unreachable_pub, dead_code)]` is the only discipline.

**Versioning.** One semver line. Any change to a `pub` item anywhere is a breaking change for everyone, including the binary's own internal users.

**Verdict.** Not SDK-consumable. The binary is the only realistic consumer.

### 2. Five-crate capability split

Layout: `mcp-server` (bin) + `code-search` (BM25 + vector + chunker) + `code-graph` (HIR + snapshot + audits) + `code-ide` (semantic / definition / refs) + `code-core` (config, errors, schema, IDs).

**SDK experience.** A CI tool depends on `code-graph = "1"` only. Pulls `heed`, `ra_ap_*`, `bincode` — does **not** pull rmcp, tokio runtime selection, tantivy, fastembed, lancedb. `code-search` is independently consumable; `code-ide` likewise. `mcp-server` is the integrator and never appears in anyone's dep tree.

**No-SDK-types rule.** Each capability crate exposes a hand-curated facade: `code_graph::Snapshot` wraps `heed::Env`; `code_graph::CallGraph` is a plain `struct { edges: Vec<CallEdge> }`. Tantivy/LanceDB types stay behind `code_search::Index` and `code_search::SearchResult`. `ra_ap_*` types stay behind `code_ide::Definition { path, line, col, name }`. The boundary is enforced by **crate visibility**: internal modules are `pub(crate)`, and the cross-crate API is small enough to review by hand. Five facades = five places to guard.

**Versioning.** Each crate ships its own semver. `code-graph 2.0` doesn't break consumers of `code-search 1.x`. The server crate pins exact minor versions of its siblings (`= "1.4"`) and bumps in lockstep when needed; external consumers float.

**Surface size.** ~5 facade types per crate × 5 crates ≈ 25 public types. Manageable. Each crate's docs.rs page is self-contained.

**Verdict.** Best-in-class SDK story. Smallest practical surface that still covers all consumer use cases.

### 3. Ten-plus crate split

Layout: every current top-level module becomes a crate (`chunker`, `embeddings`, `vector_store`, `parser`, `search`, `indexing`, `graph-extract`, `graph-storage`, `graph-queries`, `semantic`, `metadata-cache`, `config`, …).

**SDK experience.** Granular: a consumer who wants only chunking takes `chunker` (~2 deps). But "I want to search a Rust project" now requires composing `parser + chunker + embeddings + vector-store + indexing + search + config`. Six crates to version, six READMEs to read, six places where a breaking change can land.

**No-SDK-types rule.** Multiplied by 10+ facade points. `chunker::CodeChunk` is exported, but to use it with `embeddings::EmbeddingGenerator` you need a shared `Chunk` trait defined… where? Either in `code-core` (now everyone depends on core) or duplicated. Risk of leaking `tantivy::Document` from `indexing` into `search` because they share an internal type.

**Versioning.** N crates × release cadence = coordination tax. Either lock-step (defeats the point) or floating (consumers hit `code-graph-queries 2.0` requiring `code-graph-storage 1.5` requiring `code-graph-extract 1.2`).

**Verdict.** Surface area too large. Real consumers want capabilities, not microcomponents.

### 4. Hexagonal (domain core + adapters)

Layout: `code-core-domain` (pure types, no I/O, no SDKs) + `code-server-adapter` (rmcp) + `code-tantivy-adapter` + `code-lancedb-adapter` + `code-ra-adapter` + `code-heed-adapter` + `code-app` (wires them).

**SDK experience.** `code-core-domain` is the only crate a sane downstream consumer would depend on — it has zero SDK deps. But `code-core-domain` alone can't actually *do* anything: it defines `trait CodeIndex` and `trait CodeGraph` but no implementation. To search, the consumer must also pull at least one adapter, which re-introduces tantivy/lancedb. The "consumable" part is just the trait definitions.

**No-SDK-types rule.** Trivially satisfied at the core level. Adapters intentionally leak SDK types — they're the boundary. Server pins specific adapters.

**Versioning.** `code-core-domain` is the load-bearing semver: every trait change breaks every adapter. Adapters version somewhat independently but in practice move together with core.

**Verdict.** Excellent for testability and adapter swapping. Mediocre for SDK consumers, who get traits but no batteries-included implementation. Heavier than (2) without a clear consumer-side win, given we have one production adapter per port.

## Recommendation

**Adopt layout 2 — five capability crates** (`mcp-server`, `code-search`, `code-graph`, `code-ide`, `code-core`).

It uniquely satisfies both targets:

1. **MCP server use case.** `mcp-server` stays thin (rmcp router + sync manager + param structs); compile-time and binary-size cost are unchanged or slightly better thanks to crate-level parallel compilation.
2. **Future library consumers.** Each of `code-search`, `code-graph`, `code-ide` is independently usable, with a small hand-curated facade (~5 public types each) and no rmcp / tokio-runtime / SDK leakage. A CI tool can take `code-graph` alone in <30s of cold compile.

**Enforcement playbook for layout 2:**

- Mark every internal module `pub(crate)`; export only via the crate root.
- Forbid SDK types in `pub` signatures via a `cargo deny` rule on re-exports plus a CI grep for `tantivy::|lancedb::|ra_ap_|heed::|fastembed::` inside facade modules.
- Newtype every leaked-by-default ID (`NodeId`, `ChunkId`, `FileRef`) in `code-core`; downstream sees opaque values.
- `code-core` is a leaf: zero SDK deps, only `serde` + plain types. It is the only crate every other capability crate depends on.
- Each capability crate ships its own `CHANGELOG.md` and floats semver independently; the server pins exact minor versions internally.
