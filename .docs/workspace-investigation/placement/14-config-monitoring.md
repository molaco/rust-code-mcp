# 14 — Cross-cutting concerns: config, monitoring, metrics, security, schema

Anti-goal restated: no `common` / `core` / `shared` crate. Each cross-cutting concern is owned by the leaf crate that consumes it most, or by the binary if it is genuinely process-wide. Downstream crates import the owner directly; we accept a thin upward dependency rather than a kitchen-sink bucket.

## 1. `config` — owner: `mcp-server` binary crate

`Config` is a process-boot artifact: it reads env vars exactly once, derives `tantivy_dir()` / `cache_dir()`, prints a startup summary, and is then handed to subsystems as already-resolved values. That lifecycle belongs at the composition root — the `main.rs` crate that today owns `#[tokio::main]`.

What stays with the owner:
- `Config`, `default_data_dir()`, `Config::from_env()`, `print_summary()`.
- `IndexerConfig` / `IndexerCoreConfig` / `TantivyConfig` and the `for_codebase_size` tier logic (these are *built* at boot from `Config` paths).

Why not elsewhere: `indexing` would be the runner-up, but `IndexerConfig` is consumed by `search` (Tantivy budgets) and `vector_store` (GPU batch sizes) too, so pushing it down into `indexing` would force a sibling crate to depend on `indexing` purely for a config struct. The binary crate already depends on every subsystem, so it is the natural top.

What gets subsumed: `config::errors` (`ErrorContextExt`, `box_error_to_anyhow`, `is_retryable`, `ErrorMessage`) does **not** belong with `Config` — it has nothing to do with configuration. Move it to a tiny `mcp-error` crate (or fold it into `mcp-server`'s `error.rs`) that subsystem crates depend on. This is the one place where a leaf utility crate is justified because every subsystem returns `anyhow::Result`. Keep it under 200 LOC and resist growth.

## 2. `monitoring` — split, then partially delete

`monitoring` is two unrelated things bolted together. Split them and re-evaluate each.

- **`HealthMonitor`**: owner is the `mcp-server` binary. It probes `Bm25Search`, `VectorStore`, and a Merkle path — i.e. it knows about every long-lived service. That knowledge already lives in `main.rs` where `SyncManager` and `SearchTool` are wired. Make it a `health.rs` module inside `mcp-server`, served by the existing rmcp tool layer.
- **`BackupManager`**: owner is the `indexing` crate (specifically the Merkle submodule). It calls `FileSystemMerkle::save_snapshot` / `load_snapshot` directly — it is a Merkle-snapshot rotation utility wearing a generic name. Move it next to `FileSystemMerkle` and rename if useful (`merkle::snapshot::Rotator`).

Deletion candidate: **yes, the `monitoring` module name should be deleted.** It is not pulling its weight as a unit — its two halves share zero code, zero types, and zero dependencies. Keeping the umbrella forces unrelated changes through one crate boundary. Health goes to the binary; backups go to indexing; the `monitoring` crate/module ceases to exist.

## 3. `metrics` — owner: `indexing` crate

`IndexingMetrics`, `PhaseTimer`, and `MemoryMonitor` are named for their consumer: indexing. The doc admits the pipeline is "the sole expected consumer." Keep them in the `indexing` crate as `indexing::metrics`.

What stays in scope: counters, percentile math, phase timer, sysinfo memory wrapper, the single `tracing::info!` summary emit.

What about "tools also use it"? In practice, tools that want to expose indexing stats should call into `indexing` (which already owns `SyncManager` state) and let it publish a serializable snapshot — they should not be poking at raw `IndexingMetrics` fields. If a tool genuinely needs a generic stopwatch, `std::time::Instant` is two lines; do not promote `PhaseTimer` to a shared crate to save them.

`MemoryMonitor` is the only piece that is arguably general (it just wraps `sysinfo::System`). Keep it inside `indexing::metrics::memory` anyway — it has exactly one caller. If a second caller appears, copy it; we are not optimizing for theoretical reuse.

## 4. `security` — owner: `indexing` crate, with a re-export contract for `tools`

`SensitiveFileFilter` is invoked by the file walker before reading or hashing — that is squarely `indexing`. `SecretsScanner` is invoked on already-loaded text, today by ingestion; the doc notes it should *also* be used by `read_file_content` in the `tools` crate.

Owner: `indexing::security` (mirrors today's path). The `tools` crate depends on `indexing` already (via `SyncManager`), so it can call `indexing::security::SecretsScanner` directly without a new crate boundary. No `common` crate needed.

What stays in scope: the two structs, their default glob/regex sets, `should_index`, `scan`, `should_exclude`, `scan_summary`. Both types are `Send + Sync` value types with no I/O — they will not bloat `indexing`.

What does **not** belong: do not let `security` start owning auth, sandboxing, or rate-limiting. If those land later, they get their own owner (the binary, almost certainly).

## 5. `schema` — owner: `indexing` crate

`FileSchema` and `ChunkSchema` are Tantivy schema builders. The architecture doc says they are "consumed by the indexing layer" and the search layer queries fields by name. That makes `indexing` the producer; `search` is a downstream consumer that already has to depend on `indexing` for the index handle anyway.

Move `src/schema.rs` to `indexing::schema`. `search` imports `indexing::schema::{FileSchema, ChunkSchema}` — schema is not "shared," it is **owned by the writer and read by the reader**, which is the standard Rust workspace pattern.

Why not `mcp-server`: schemas are not process-boot config; they are persistence contracts that change with indexing logic. Coupling them to the indexing crate means schema migrations and indexer rewrites land in the same PR, which is correct.

What stays in scope: the two `Schema` constructors and their field accessors. Nothing else. Do not let `schema` grow to own serde DTOs or tool-response shapes — those belong with their respective tools.
