# Cross-cutting Policies

Operations manual for the `rust-code-mcp` workspace. Authoritative source: [`DECISIONS.md`](../DECISIONS.md). Where this document and DECISIONS conflict, DECISIONS wins; open a PR to fix this file.

---

## 1. Cargo workspace structure

### Root `Cargo.toml` (virtual manifest)

```toml
[workspace]
resolver = "3"
members = [
    "crates/rcm-paths",
    "crates/rcm-ra-syntax",
    "crates/rcm-ra-host",
    "crates/rcm-embedding",
    "crates/rcm-search",
    "crates/rcm-graph",
    "crates/rcm-ide",
    "crates/rcm-server",
    "crates/xtask",
]
# xtask is a member but is excluded from the runtime architecture policies
# enforced in CI (forbidden_dependency_check, public-api leak, missing_docs,
# unreachable_pub). See `xtask/Cargo.toml` metadata + ci/policy.toml.

[workspace.package]
edition      = "2024"
rust-version = "1.85"
license      = "MIT OR Apache-2.0"
authors      = ["rcm contributors"]
repository   = "https://github.com/<org>/rust-code-mcp"
publish      = false

[workspace.dependencies]
# Async runtime + utilities
tokio          = { version = "1.40", features = ["rt-multi-thread", "macros", "sync", "signal", "time", "fs"] }
tokio-util     = { version = "0.7", features = ["rt"] }
futures        = "0.3"
async-trait    = "0.1"

# Errors / serde / tracing
thiserror      = "1.0"
anyhow         = "1.0"
serde          = { version = "1.0", features = ["derive"] }
serde_json     = "1.0"
tracing        = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }

# Concurrency primitives
arc-swap       = "1.7"
parking_lot    = "0.12"
dashmap        = "6.1"

# Hashing / paths / IO
sha2           = "0.10"
directories    = "5.0"
walkdir        = "2.5"
ignore         = "0.4"
memmap2        = "0.9"

# Storage / search backends (only consumed by capability/infra crates)
tantivy        = "0.22"
lancedb        = "0.10"
arrow          = "52"
heed           = "0.20"
sled           = "0.34"

# Embeddings
fastembed      = "4"
ort            = "2.0.0-rc.4"

# rust-analyzer crates (pinned; never used outside rcm-ra-syntax / rcm-ra-host)
ra_ap_syntax   = "0.0.241"
ra_ap_ide      = "0.0.241"
ra_ap_hir      = "0.0.241"
ra_ap_load-cargo = "0.0.241"
ra_ap_vfs      = "0.0.241"

# MCP server (only consumed by rcm-server)
rmcp           = { version = "0.2", features = ["server", "transport-io"] }

# Test deps (used via dev-dependencies)
proptest       = "1.5"
insta          = "1.40"
assert_matches = "1.5"
tempfile       = "3.12"

[workspace.lints.rust]
unsafe_op_in_unsafe_fn = "deny"
unreachable_pub        = "warn"
rust_2024_idioms       = { level = "warn", priority = -1 }
# missing_docs is NOT workspace-wide; applied per-crate (§2).

[workspace.lints.clippy]
pedantic           = { level = "warn", priority = -1 }
disallowed_methods = "warn"
# .clippy.toml at the workspace root lists `RaHost::with_db` and
# `RaHost::with_semantics` as disallowed_methods. Note: clippy does NOT
# support caller-crate-specific allow-lists. The actual enforcement is:
#   1. The lint fires globally for any caller of these methods.
#   2. `rcm-graph` and `rcm-ide` annotate their call sites with
#      `#[allow(clippy::disallowed_methods)]` and a `// SAFETY:` justification.
#   3. PR review rejects new `#[allow(...)]` annotations outside those two
#      crates. A trivial CI grep enforces this:
#      `! grep -r '#\[allow(clippy::disallowed_methods)\]' \
#         --include='*.rs' crates/rcm-{search,server,paths,embedding,ra-host,ra-syntax}`
# This is discipline + lint + grep, not a clippy feature.

[profile.release]
lto       = "thin"
codegen-units = 1
strip     = "symbols"

[profile.dev]
# Tantivy / ort cold builds dominate dev iteration; opt for compile speed.
debug = "limited"
```

### `rust-toolchain.toml`

```toml
[toolchain]
channel    = "1.85.0"
components = ["rustfmt", "clippy", "rust-src", "rust-analyzer"]
profile    = "minimal"
```

### `deny.toml`

```toml
[advisories]
db-path  = "~/.cargo/advisory-db"
db-urls  = ["https://github.com/rustsec/advisory-db"]
yanked   = "deny"
ignore   = []  # Empty by policy; every ignore needs a tracking issue + expiry.

[licenses]
confidence-threshold = 0.9
allow = [
  "MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception",
  "BSD-2-Clause", "BSD-3-Clause", "ISC", "Unicode-DFS-2016",
  "Zlib", "MPL-2.0", "CC0-1.0",
]
exceptions = []

[bans]
multiple-versions = "warn"   # Tantivy + arrow pull duplicates today; deny later.
wildcards         = "deny"
deny = [
  # Deprecated / superseded crates we never want to introduce.
  { name = "openssl",        reason = "use rustls" },
  { name = "failure",        reason = "use thiserror/anyhow" },
  { name = "error-chain",    reason = "use thiserror" },
  { name = "lazy_static",    reason = "use std::sync::LazyLock or once_cell" },
]
skip = []
skip-tree = []

[sources]
unknown-registry = "deny"
unknown-git      = "deny"
allow-registry   = ["https://github.com/rust-lang/crates.io-index"]
allow-git        = []
```

---

## 2. Per-crate lint application

Each crate's `lib.rs` (or `main.rs` for the binary) starts with the lints below. Workspace-wide lints from §1 already apply; this table lists what each crate **adds**.

| Crate           | `missing_docs` | `unreachable_pub` | `rustdoc::broken_intra_doc_links` | Notes |
|-----------------|:--:|:--:|:--:|---|
| `rcm-paths`     | warn | inherit (warn) | warn | strict API leak tier |
| `rcm-ra-syntax` | -    | inherit (warn) | warn | doc the re-export whitelist |
| `rcm-ra-host`   | -    | inherit (warn) | warn | document `with_db` allow-list |
| `rcm-embedding` | warn | inherit (warn) | warn | doc the sealed-trait policy |
| `rcm-search`    | warn | inherit (warn) | warn | strict API leak tier |
| `rcm-graph`     | warn | inherit (warn) | warn | strict API leak tier |
| `rcm-ide`       | warn | inherit (warn) | warn | strict API leak tier |
| `rcm-server`    | -    | inherit (warn) | warn | docs are README-driven |
| `xtask`         | -    | **allow**       | warn | excluded from runtime policy |

Example crate root for a capability crate (`crates/rcm-search/src/lib.rs`):

```rust
#![warn(missing_docs)]
#![warn(rustdoc::broken_intra_doc_links)]
#![doc = include_str!("../README.md")]
```

`clippy::disallowed_methods` is workspace-level. The DENY-LIST (NOT an allow-list — clippy has no caller-crate-specific allow-list mechanism) lives in `.clippy.toml`:

```toml
# .clippy.toml — names methods that are forbidden by default for ALL callers.
disallowed-methods = [
    { path = "rcm_ra_host::RaHost::with_db",
      reason = "Closure-based RA-host internals; misuse risks lifetime leaks of RootDatabase.",
      allow-invalid = false },
    { path = "rcm_ra_host::RaHost::with_semantics",
      reason = "Same boundary as with_db." },
]
```

The lint then fires globally on every call site. Permission is granted **per call site** in `rcm-graph` and `rcm-ide` via:

```rust
#[allow(clippy::disallowed_methods)]   // SAFETY: rcm-graph extraction owns this RA load; closure scope is correct.
host.with_db(|db| { /* ... */ });
```

The actual workspace-level enforcement is a CI grep that rejects this annotation outside the two trusted crates:

```bash
! grep -r '#\[allow(clippy::disallowed_methods)\]' \
    --include='*.rs' \
    crates/rcm-search crates/rcm-server crates/rcm-paths \
    crates/rcm-embedding crates/rcm-ra-host crates/rcm-ra-syntax
```

Discipline + lint + grep, not a clippy feature. Anyone wanting to bypass the boundary must pass code review on adding either a new entry to `.clippy.toml` or a new `#[allow(...)]` site in `rcm-graph`/`rcm-ide`.

---

## 3. Error taxonomy

Capability crates use **operation-scoped** typed errors. No god-enum. `rcm-server` uses `anyhow` for top-level orchestration where the caller will only report and abort.

| Crate           | Error enums (operation-scoped)                          | Style          |
|-----------------|---------------------------------------------------------|----------------|
| `rcm-paths`     | `PathError`                                             | `thiserror`    |
| `rcm-ra-syntax` | (re-exports only; no error type)                        | -              |
| `rcm-ra-host`   | `RaError`                                               | `thiserror`    |
| `rcm-embedding` | `EmbedError` (`ModelInit`, `EmbedFailed`, `Empty`, `Join`, `Unsupported`) | `thiserror` |
| `rcm-search`    | `IndexError`, `SearchError`, `CorpusError`              | `thiserror`    |
| `rcm-graph`     | `BuildError`, `QueryError`, `AuditError`                | `thiserror`    |
| `rcm-ide`       | `IdeError`                                              | `thiserror`    |
| `rcm-server`    | `anyhow::Error` for orchestration; tool handlers map domain errors → JSON-RPC | `anyhow` |

Example showing `#[from]` source preservation:

```rust
// crates/rcm-search/src/error.rs
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IndexError {
    #[error("workspace not tracked: {0}")]
    NotTracked(PathBuf),

    #[error("tantivy writer busy; another batch is in progress")]
    IndexBusy,

    #[error("tantivy I/O error")]
    Tantivy(#[from] tantivy::TantivyError),

    #[error("metadata cache failure")]
    Cache(#[from] sled::Error),

    #[error("embedder failed during indexing")]
    Embed(#[from] rcm_embedding::EmbedError),

    #[error("filesystem walk failed")]
    Walk(#[from] ignore::Error),
}
```

Rules:

- Capability errors implement `std::error::Error` via `thiserror`; sources flow through `#[from]` / `#[source]`.
- No `Display` interpolation of secret paths or tokens — wrap sensitive fields.
- `rcm-server` calls `.context("during index_codebase")` at the rmcp tool boundary and returns `anyhow::Error`.
- Tests may use `anyhow` and `assert_matches!` for ergonomic assertions.

---

## 4. Service lifetime + invalidation

Reproduced from DECISIONS with operational elaboration.

| Resource                         | Lifetime           | Reload mechanism                                                |
|----------------------------------|--------------------|-----------------------------------------------------------------|
| `tantivy::IndexReader`           | long-lived         | `ArcSwap<IndexReader>` — open new, swap, drop old after grace   |
| `tantivy::IndexWriter`           | long-lived         | `Mutex<IndexWriter>` — single-writer; reload returns `IndexBusy` if writing |
| `lancedb::Connection`            | long-lived         | `ArcSwap<Connection>`                                           |
| `sled::Db`                       | construction-time  | rebuild service to reload                                       |
| `heed::Env` (per `OpenedSnapshot`) | per-snapshot     | `ArcSwap<OpenedSnapshot>` — atomic `CURRENT` swap               |
| `RaHost` (IDE cache)             | long-lived         | `ArcSwap<HashMap<PathBuf, Arc<RaHost>>>`                        |
| `RaHost` (graph build)           | per-call           | scoped to `build_and_persist`                                   |
| `Embedder`                       | long-lived         | constructed once in `main`, `Arc<dyn Embed>` everywhere         |
| `SyncManager` task               | long-lived         | `CancellationToken`-driven shutdown                             |

### Reload sequence (ArcSwap pattern)

```text
fn reload_index(svc: &SearchService, ws: &Path) -> Result<(), IndexError> {
    let new_reader = open_reader(&svc.tantivy_index_at(ws))?; // 1. open new
    let prev = svc.reader.swap(Arc::new(new_reader));         // 2. atomic swap
    drop(prev);                                               // 3. last Arc drop releases mmap
    // In-flight readers holding `Guard`s drain naturally; ArcSwap is wait-free.
    Ok(())
}
```

`reload` returns `IndexError::IndexBusy` when `writer.try_lock()` fails — callers retry or wait.

### SIGINT shutdown sequence

```text
SIGINT (or stdin EOF on stdio transport)
  -> CancellationToken::cancel()
  -> SyncManager::run select! arm fires, loop exits cleanly
  -> drain_inflight_tools(deadline = now + 30s)
       (handlers observe ct, finish current write, return)
  -> drop in topological order:
       drop(server)           // releases tool router, rmcp transport
         drop(rcm_ide)        // releases IDE RaHost cache
         drop(rcm_graph)      // closes heed Env, drops snapshots
         drop(rcm_search)     // commits tantivy, closes lancedb, flushes sled
         drop(rcm_embedding)  // joins ort/fastembed worker, releases CUDA
         drop(rcm_ra_host)    // drops RootDatabase + Vfs
         drop(rcm_paths)      // pure data; no cleanup
  -> on deadline expiry: tracing::error!(remaining = N, "drain timeout"); abort.
```

**30-second drain rationale.** Tantivy `commit()` on a 500-file batch typically completes in under 5s on cold disks; LanceDB writes are bounded by an open-segment flush. 30s gives both a safety margin without becoming a UX problem on a forced shutdown. If repeated drain timeouts appear in the field, raise to 45s and revisit batch sizing — do not silently extend.

### Invalidation triggers (verbatim from DECISIONS)

| Trigger                              | Action                                                                 |
|--------------------------------------|------------------------------------------------------------------------|
| `clear_cache(workspace, scope)`      | (1) Refuse with `IndexBusy` if a writer is mid-batch. (2) `rm -rf` the on-disk paths for the scope. (3) `SearchService::invalidate(workspace)`, `GraphService::invalidate(workspace)`, `IdeService::evict(workspace)` — drop handles only; do NOT reload, do NOT auto-reindex. The next op rebuilds via the existing fingerprint-mismatch path. |
| Workspace fingerprint mismatch       | Transparent rebuild via `build_and_persist`.                           |
| `SyncManager::sync_now` schema change| `SearchService::reload(workspace)` — opens new handles, `ArcSwap` swap, drops old after grace. No on-disk delete. |
| SIGINT / stdin EOF                   | See sequence above.                                                    |
| `track_directory(new)`               | No reload; `SearchService` opens lazily on first `search`.             |

---

## 5. Async boundary policy

| Crate           | Tokio dep?              | Why                                                          |
|-----------------|:-----------------------:|--------------------------------------------------------------|
| `rcm-paths`     | NO                      | Pure path math + hashing.                                    |
| `rcm-ra-syntax` | NO                      | Re-export shim, sync only.                                   |
| `rcm-ra-host`   | NO                      | Sync RootDatabase wrapper; callers `spawn_blocking`.         |
| `rcm-embedding` | YES (limited)           | `embed_batch_async` is the **single** place embedding hits tokio. Sync `Embed` trait + sync impl; async wrapper uses `tokio::task::spawn_blocking`. |
| `rcm-search`    | YES                     | Service methods are async; sans-I/O cores (`chunker`, `rrf`) are sync. |
| `rcm-graph`     | YES                     | Service methods async; audit cores sync.                     |
| `rcm-ide`       | YES                     | Service methods async; calls `RaHost` via `spawn_blocking`.  |
| `rcm-server`    | YES                     | Owns the runtime; `#[tokio::main]` lives here.               |
| `xtask`         | YES (incidental)        | Excluded from policy.                                        |

### CI check sketch (`xtask forbidden-dependency-check`)

```rust
// crates/xtask/src/forbidden_deps.rs (abbreviated)
const SYNC_ONLY: &[&str] = &[
    "rcm-paths", "rcm-ra-syntax", "rcm-ra-host",
];
const SEARCH_FORBIDDEN: &[&str] = &["rcm-graph", "rcm-ide"];
const GRAPH_FORBIDDEN:  &[&str] = &["rcm-search", "rcm-ide"];
const IDE_FORBIDDEN:    &[&str] = &["rcm-search", "rcm-graph"];

fn check(meta: &cargo_metadata::Metadata) -> Result<(), Violation> {
    for pkg in &meta.workspace_packages() {
        if SYNC_ONLY.contains(&pkg.name.as_str())
            && pkg.dependencies.iter().any(|d| d.name == "tokio")
        {
            return Err(Violation::TokioInSyncCrate(pkg.name.clone()));
        }
        // ... capability cross-deps ...
    }
    Ok(())
}
```

The check runs in CI as `cargo xtask forbidden-dependency-check`; failure is blocking.

---

## 6. Observability

- `tracing` is the single observability surface across crates. No `log` crate calls in new code; bridge external `log` users with `tracing-log`.
- Every async service method on a capability crate carries `#[tracing::instrument(skip(self), fields(workspace = %ws.display()))]` (or equivalent). Span names are `<crate>.<method>` (e.g. `search.run`, `graph.who_calls`).
- **Structured fields, not interpolated strings.** `tracing::info!(target_kb = bytes / 1024, "tantivy commit")`, never `tracing::info!("commit {bytes}")`.
- **No span guards across `.await`.** Always `.instrument(span)`. The clippy lint `clippy::let_underscore_future` is enforced; reviewers reject `_g = span.enter()` near async code.
- **Production output:** JSON via `tracing_subscriber::fmt().json()` writing to stderr (stdio transport keeps stdout clean for rmcp). Filter with `RUST_LOG`; default `info,rcm=debug`.
- **Dev output:** human-readable via `tracing_subscriber::fmt::layer()`.

### Typed metrics

`IndexingMetrics` (defined in `rcm-search`) is the **only** typed metric struct. It is a per-batch summary:

```rust
#[derive(Debug, Clone, serde::Serialize)]
pub struct IndexingMetrics {
    pub files_seen: u64,
    pub files_indexed: u64,
    pub files_skipped_secrets: u64,
    pub bytes_indexed: u64,
    pub elapsed: std::time::Duration,
}
```

Everything else — call counts, latencies, queue depths, cache hits, embedding throughput — is exposed as `tracing::Span` fields (`record(...)`) or `tracing::event!`. We do not maintain a `MetricsRegistry`, Prometheus handle, or counter-of-counters; if external metrics become a requirement, add a `tracing-opentelemetry` layer at the `rcm-server` composition root.

---

## 7. Supply chain

### `cargo-deny` rationale

- **advisories**: gates the build on RustSec advisories; `yanked = "deny"` so we never pin a yanked release.
- **licenses**: explicit allow-list. Anything outside requires a PR amending `deny.toml` plus a justification; this prevents accidental copyleft contamination from a transitive dep.
- **bans.multiple-versions = "warn"**: today tantivy/arrow drag duplicate transitive crates; we warn now and tighten to `deny` after Phase 1 stabilizes.
- **bans.deny**: pre-empts known-bad swaps (`openssl` → `rustls`, `lazy_static` → `LazyLock`).
- **sources**: only crates.io, no git deps — git deps would defeat reproducibility.

### `Cargo.lock` policy

`Cargo.lock` **is committed**. The workspace ships a binary (`rcm-server`) and we want byte-reproducible release builds. Any consumer that links us as a library is responsible for its own lockfile.

### MSRV policy

No external SDK consumers exist today. We pin `rust-version = "1.85"` in `[workspace.package]` to match `rust-toolchain.toml`, but make no MSRV stability promise yet. **If/when** a public crate is published or external consumers materialize, we will:

1. Set MSRV one stable behind the toolchain.
2. Add a `cargo +<MSRV> check --workspace` job to CI.
3. Document the MSRV in each public crate's README and bump it via a major version.

### Dependency review checklist (PR template addition)

Before adding a new external crate, reviewer confirms:

- [ ] Is there an existing workspace dep that fits? (`workspace = true` first.)
- [ ] Maintainership: last release < 18 months, issues being triaged.
- [ ] License is on the allow-list in `deny.toml`.
- [ ] No proc-macro / build-script unless justified.
- [ ] Transitive surface inspected with `cargo tree -e features`.
- [ ] Pinned in `[workspace.dependencies]`, not in the consuming crate.
- [ ] Capability crates: does adding this leak into a public signature? If yes, refactor.

---

## 8. CI gate matrix

| Policy                                      | Enforcement mechanism                                                  | Blocking? |
|---------------------------------------------|------------------------------------------------------------------------|:---------:|
| Forbidden dependency graph (cap ⊥ cap, sync-only crates)        | `cargo xtask forbidden-dependency-check` (reads `cargo metadata`)      | block     |
| Public-API leak (no `tantivy::`, `lancedb::`, `arrow::`, `fastembed::`, `ort::`, `ra_ap_*`, `heed::`, `sled::`, `rmcp::` in capability+`rcm-paths` exports) | `cargo public-api --diff-git-checkouts <base> <head>` + workspace grep over generated API JSON | block     |
| `missing_docs` on capability + selected leaves | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`        | block     |
| `unreachable_pub` everywhere except xtask   | `cargo clippy --workspace -- -D warnings` (lint configured per §2)     | block     |
| `rustdoc::broken_intra_doc_links`           | Same `cargo doc` job above                                             | block     |
| `cargo-deny` advisories + licenses + bans   | `cargo deny check advisories licenses bans sources`                    | block     |
| `clippy::disallowed_methods` (RaHost deny-list + per-site `#[allow]` exceptions enforced by CI grep) | `cargo clippy --workspace -- -D warnings` plus the grep in §2 | block     |
| `unsafe_op_in_unsafe_fn = "deny"`           | `cargo build --workspace` (deny lint = compile error)                  | block     |
| Smoke checklist (10 MCP calls vs. fixture)  | `cargo xtask smoke` against `fixtures/sample-workspace`                | block     |
| `cargo test --workspace`                    | nextest                                                                | block     |
| `multiple-versions` duplicate dep audit     | `cargo deny check bans`                                                | warn      |
| Bench / perf regression                     | `cargo xtask bench` (manual, scheduled)                                | warn      |

Smoke checklist (the `cargo xtask smoke` job exercises against `fixtures/sample-workspace`):

`index_codebase` → `search` → `find_definition` → `find_references` → `build_hypergraph` → `who_calls` / `who_imports` / `workspace_stats` → `get_dependencies` / `get_call_graph` / `analyze_complexity` → `semantic_overlaps` (when `embeddings` on) → `similar_to_item` → `clear_cache(ws)` then `search` (verifies on-disk delete + handle invalidation + lazy rebuild on next read).

Failures in any blocking gate block merge. Warning gates surface in PR comments via the CI bot but do not block; persistent warnings become tracked issues.
