# 04 — Minimal Split

## Why split at all (or not)

A workspace split is only justified when it solves a *real* problem. The candidate problems for `file-search-mcp` and whether they actually exist:

- **Compile time.** Real. `Cargo.toml` pulls in `ra_ap_*` (9 crates), `tantivy`, `lancedb` + `arrow-*`, `fastembed` + `ort` (CUDA ONNX), `heed`, `sled`, `rmcp`. A clean `cargo build` is dominated by these. But splitting the *source* crate doesn't make the *dependency* crates compile faster; it only helps incremental rebuilds when edits land in a leaf crate that nothing else depends on. Concrete win is modest unless we isolate a leaf that churns.
- **Testability.** Partially real. Per the project memory, snapshot builds cost ~115 s and `cargo test` is discouraged. A leaf crate with its own dep graph (no `ra_ap_ide`, no `ort`, no `lancedb`) could run `cargo test -p` in seconds. The pure-data modules (`parser`, `chunker`, `security`, `metrics`, `config/errors`) qualify.
- **Distinct release cadences.** Not real. One binary, one consumer (an MCP client), no published library, no semver contract.
- **Consumer-facing SDK.** Not real. `lib.rs` exists only to host `main.rs` and `bin/test_tools_direct.rs`. There is no external consumer.
- **Forbidden-dependency enforcement.** Weak. The architecture already keeps tiers clean by convention; no current bug points to a layering violation that only Cargo can catch.

The only paid cost today is **incremental rebuild latency for fast-iterating modules** (parser/chunker/audits) being coupled to `ort`/`lancedb`/`ra_ap_ide` link time. Everything else ("better architecture", "cleaner boundaries") is aesthetic.

## Proposed crate count + names

**Two crates.** Not three. Not five.

1. `rust-code-core` (lib) — pure-Rust, AST-only, no heavy native deps. Holds: `parser`, `chunker`, `security`, `metrics`, `config` (the parts free of `directories`/`sled`), `schema`, error types. Depends only on `ra_ap_syntax`, `serde`, `regex`, `glob`, `sha2`, `thiserror`, `tracing`.
2. `file-search-mcp` (bin + the rest of lib) — everything else: `embeddings`, `vector_store`, `indexing`, `search`, `semantic`, `graph`, `tools`, `mcp`, `monitoring`, `metadata_cache`, `main.rs`. Keeps `ort`, `lancedb`, `ra_ap_ide`, `heed`, `sled`, `rmcp`, `tokio`.

## Dependency graph

```
file-search-mcp (bin + lib)
        │
        ▼
  rust-code-core (lib)
```

One edge. No cycles possible. No third crate needed.

## What's gained

- `cargo check -p rust-code-core` and `cargo test -p rust-code-core` finish in seconds: AST/regex/chunking changes — the most-edited area per the recent commit log (channel/derive/docs/fn-body/recursion audits, `ast_resolve`) — stop pulling `ort` and `lancedb` into the link step.
- A single, mechanically enforced rule: "core knows nothing about Tantivy, LanceDB, ONNX, rmcp, tokio." If someone tries to use `tokio` from `parser`, Cargo refuses.
- `bin/test_tools_direct.rs` and pure-AST examples can move to the core crate and run without booting the heavy stack.

## What's NOT gained

- No faster *cold* build — the heavy deps are still pulled by the bin crate and dominate first-build time.
- No release-cadence flexibility (still one shipped artifact).
- No public-API stability guarantee (still no external consumer).
- No improved runtime behavior — concurrency, memory, latency are unchanged.
- No clearer mental model for the *upper* tiers (search, indexing, graph, semantic) — they remain a single crate because splitting them buys nothing and costs `pub` surface, `Cargo.toml` churn, and longer link times from extra rlib boundaries.

## Top 3 weaknesses

1. **`config` is hard to bisect cleanly.** `IndexerConfig`/`TantivyConfig` reference Tantivy types, but `errors.rs` and the env-loading skeleton are pure. Splitting will require either pulling a small `config-core` portion down or accepting some duplication of error types. Likely a half-day of de-tangling.
2. **`schema` lives in core but uses `tantivy::schema::*`.** Either `tantivy` becomes a (light) dep of core — undermining the whole point — or `ChunkSchema`/`FileSchema` stay in the bin crate and core only owns chunk *data structs*. The latter is correct but means a small re-org of `chunker`'s output types.
3. **Two crates is the floor, not a stepping stone.** Teams that split into 2 often rationalize a slide to 5–8 ("now that we have a workspace…"). The discipline to *stop here* is the actual hard part, and there's no Cargo mechanism that enforces "only two."

## Counter to "more crates = better architecture"

Each additional crate adds: a `Cargo.toml`, a `pub` API surface that must be maintained, an rlib link step, cross-crate generic monomorphization that the compiler can no longer inline cheaply, and a place for circular-dep refactors to get stuck. The `ra_ap_*` ecosystem is the cautionary tale: 9 crates here, glacial to compile, painful to navigate. Module boundaries inside one crate are *free* and `#![warn(unreachable_pub)]` (already enabled in `lib.rs`) gives 80 % of the encapsulation benefit at 0 % of the cost.

## When this is the right choice

Pick the 2-crate split iff **all** hold: (a) one module cluster has visibly lighter deps than the rest, (b) that cluster is where most edits land, (c) you have measured (not guessed) that incremental rebuilds are the bottleneck, (d) there is no public SDK story forcing a different shape. For `file-search-mcp` (a) and (b) hold; (c) is plausible given the snapshot-build cost; (d) is satisfied. Two crates. Stop.
