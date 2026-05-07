# Megalodon Plan — Workspace Migration

Single-source, self-contained migration plan for splitting the current
`file-search-mcp` single-crate project into an 8-crate Cargo workspace.

This document supersedes ad-hoc references. The detailed per-phase docs
under `.docs/workspace-plan/` remain as background; this file is the one
to read end-to-end before implementing.

---

## 1. Goal

Take the current single-crate Rust MCP server (Tantivy + LanceDB +
fastembed/ort + ra-analyzer + heed/sled, exposing ~50 MCP tools over
stdio) and split it into a small workspace where:

- **Capability boundaries are enforceable**, not aspirational. A capability
  crate can be consumed by SDK users without dragging in rmcp, tokio
  runtime, or other capabilities' backends.
- **Hidden globals are removed.** Services are constructed once in `main`
  and passed by `Arc`; no `LazyLock<Mutex<…>>` runtime statics survive.
- **The embedding lifecycle is explicit.** One ONNX session, shared by
  search and graph through a sealed trait, with a deterministic test fake.
- **Storage migration is deferred** to a separate phase; the structural
  split lands first and is independently shippable.

Not goals: adding a second backend, a new transport, multi-tenancy,
cosmetic crate renames.

---

## 2. Target architecture

### Crate inventory

| Crate | Type | Purpose |
|---|---|---|
| `rcm-paths` | infra leaf | `ProjectPaths`, `StorageRoot { Xdg, Explicit }`, and a single owner for the workspace-hash recipe(s). **Phase 1 preserves the legacy raw-path-string recipe verbatim** to avoid orphaning existing on-disk indexes; canonicalization (and any unification across today's three divergent hash sites in `tools/project_paths.rs`, `graph/ids.rs`, `indexing/incremental.rs`) is a Phase 7 decision, not Phase 1. |
| `rcm-ra-syntax` | infra leaf | Narrow, whitelisted re-exports of `ra_ap_syntax` items used by the chunker. Centralizes RA version pinning. |
| `rcm-ra-host` | infra leaf | `RaHost` lifecycle wrapper around `RootDatabase` + `Vfs`. Two presets: `open_ide` (light, IDE) and `open_hir` (heavy, snapshot build). Closure-based access to `&RootDatabase`. |
| `rcm-embedding` | infra leaf | Sealed `Embed` trait + `Embedder = Arc<dyn Embed>`. Production `FastEmbedEmbedder` (feature `embeddings`, default-on) and `DeterministicEmbedder` (feature `test-fakes`, default-off). |
| `rcm-search` | capability | Corpus + retrieval: chunker (uses `ra-syntax` for context only), Tantivy, LanceDB, sled metadata cache, hybrid RRF. `SearchService` + `CorpusWriter`. |
| `rcm-graph` | capability | Persisted hypergraph: HIR extraction, heed snapshot, queries, audits, file-scoped structural tools (`get_dependencies`/`get_call_graph`/`analyze_complexity`). Embedder is optional via builder. |
| `rcm-ide` | capability | Live navigation: `IdeService::open(paths)` (no statics), `find_definition`, `find_references`, `symbol_search`. |
| `rcm-server` | bin + thin lib | rmcp router, `*Params`, `Config`, `SyncManager` shell, service composition, `similar_to_item`, error mapping. The only crate that depends on rmcp/serde-json/tokio macros. |
| `xtask` | tooling | Workspace automation (forbidden-deps script, storage v2 migration, smoke checklist). Excluded from runtime architecture policies. |

### Dependency graph

```
rcm-server  ─►  { rcm-search, rcm-graph, rcm-ide, rcm-paths, rcm-embedding }
rcm-search  ─►  { rcm-ra-syntax, rcm-embedding (default-features=false), rcm-paths }
rcm-graph   ─►  { rcm-ra-host, rcm-embedding (feature semantic-overlaps), rcm-paths }
rcm-ide     ─►  { rcm-ra-host, rcm-paths }
rcm-ra-host ─►  { rcm-ra-syntax }

FORBIDDEN: capability ↔ capability edges. Enforced by the forbidden-deps
script (Phase 0). xtask is excluded from this policy.
```

### Why 8, not 5 or 11

- Capability boundaries match the three things the binary actually does
  (corpus, graph, IDE).
- Infra leaves isolate the three external toolchains (`ra_ap_syntax`,
  `ra_ap_ide`/`ra_ap_hir`, `fastembed`/`ort`) that were tangled in the
  monolith.
- Five crates collapse `rcm-paths` into every consumer (drift on the
  hash recipe) and force `rcm-server` to know XDG layout.
- Eleven crates recreate the backend-keyed split the original proposal
  rejected, with no compile-time win to show for it.

---

## 3. Load-bearing cross-cutting decisions

Every phase below assumes these. They are the contract.

**3.1. `clear_cache` is delete-then-invalidate, NOT reload — and lazy
rebuild differs by capability.** Three steps, in order:

1. Refuse with `IndexBusy` if a writer is mid-batch.
2. `rm -rf` the on-disk artifacts for the requested scope (`workspace`
   default; per-area scopes `keyword | vector | metadata | merkle | graph
   | all` available later).
3. Call `SearchService::invalidate(workspace)`,
   `GraphService::invalidate(workspace)`, `IdeService::evict(workspace)`
   to drop in-memory handles.

**Rebuild behavior is asymmetric.** `search` already lazily rebuilds on
stale-index detection (`tools::query_tools::search` calls
`UnifiedIndexer::ensure_indexed` today), so `clear_cache` followed by
`search` works with no user action. **Graph queries do not auto-rebuild
today** — `tools::graph_tools::*` returns "call `build_hypergraph`
first" when the snapshot is missing. Phase 4 preserves this asymmetry:
after `clear_cache`, the user must explicitly call `build_hypergraph`
before the next graph query. Symmetry (graph auto-rebuilds on
not-found) is an explicit non-goal of Phase 4; if wanted later, it gets
its own phase. There is no auto-reindex on clear.

**3.2. Sealed `Embed` trait, not concrete `Embedder`.** Capability crates
take `Embedder = Arc<dyn Embed>`. Production impl is `FastEmbedEmbedder`
behind feature `embeddings`. Test impl is `DeterministicEmbedder` behind
feature `test-fakes`. Sealed so external crates cannot break the
dimensionality contract.

**3.3. `RaHost::with_db` is a closure boundary, not a friend-crate
trick.** Rust has no friend crates. The methods are `pub`. The
`disallowed_methods` lint fires globally; only `rcm-graph` and `rcm-ide`
annotate call sites with `#[allow(clippy::disallowed_methods)]` plus a
one-line justification. A CI grep rejects the same `#[allow]` outside
those two crates.

**3.4. Long-lived services with `ArcSwap` reload.** Services constructed
once in `rcm-server::main`, passed by `Arc`. Reload is per-resource:

| Resource | Mechanism |
|---|---|
| Tantivy `IndexReader` | `ArcSwap<IndexReader>` — open new, swap, drop old after grace |
| Tantivy `IndexWriter` | `Mutex<IndexWriter>` — single-writer; `clear_cache` returns `IndexBusy` mid-batch |
| LanceDB `Connection` | `ArcSwap<Connection>` |
| sled `Db` | construction-time only |
| heed `Env` (per `OpenedSnapshot`) | `ArcSwap<OpenedSnapshot>` |
| `RaHost` (IDE cache) | `ArcSwap<HashMap<PathBuf, Arc<RaHost>>>` |
| `Embedder` | constructed once in `main`, `Arc<dyn Embed>` shared |

`SyncManager::reload(workspace)` (NOT `clear_cache`) is the only path
that opens new handles eagerly without deleting on-disk data. Used after
schema-version bumps detected mid-sync.

**3.5. Two-tier API leak rule.**

- **Strict tier** (`rcm-search`, `rcm-graph`, `rcm-ide`, `rcm-paths`): public
  signatures may NOT contain `tantivy::`, `lancedb::`, `arrow::`,
  `fastembed::`, `ort::`, `ra_ap_*`, `heed::`, `sled::`, or `rmcp::`.
  Enforced by `cargo public-api` CI grep.
- **Exempt tier** (`rcm-ra-syntax`, `rcm-ra-host`, `rcm-embedding`): the
  re-exports / closure args / sealed trait ARE the boundary. Each leaf
  crate's root doc declares its exemption explicitly.

**3.6. Async at the boundary.** `rcm-paths`, `rcm-ra-syntax`,
`rcm-ra-host`, sans-I/O cores (chunker, RRF, audits, queries) — no
tokio. `rcm-embedding`'s sync core has async wrappers (the one place to
`spawn_blocking`). Capability service methods are async; their domain
cores are sync. `rcm-server` owns `#[tokio::main]`.

**3.7. Operation-scoped errors.** Each capability crate owns its own
`thiserror`-derived enums (`SearchError`, `IndexError`, `CorpusError`,
`BuildError`, `QueryError`, `AuditError`, `IdeError`, `EmbedError`,
`PathError`, `RaError`). `anyhow` is confined to `rcm-server`. No god-enum.

**3.8. DTOs are not `Serialize`.** Capability-crate public types
(`SearchHit`, `Node`, `Definition`, etc.) are not `Serialize` /
`Deserialize`. Only `rcm-server` serializes — it owns the wire format.
Mirrors rust-analyzer's pattern; prevents JSON-RPC shape from
constraining domain types.

**3.9. Workspace tooling.** Virtual `Cargo.toml` with
`[workspace.package]` (edition `2024`), `[workspace.dependencies]`
(every external dep pinned in one place), `[workspace.lints.rust]` and
`[workspace.lints.clippy]`. `Cargo.lock` committed. `rust-toolchain.toml`
pins `1.95.0` (edition 2024 + resolver "3"). `cargo-deny`,
`cargo-public-api`, and the forbidden-deps xtask script are CI gates.
File-based modules only (`parser.rs` + `parser/lexer.rs`); no `mod.rs`
in new code.

---

## 4. Phases

Each phase keeps `cargo build --workspace` green and the smoke checklist
(end of doc) passing. Each phase is independently revertible until
explicitly noted otherwise.

### Phase 0 — Workspace skeleton

**Goal.** Establish the workspace shell and CI policy gates. No
production code moves; new placeholder crates are empty.

**Important caveats up front.** Phase 0 is "shell-only" only because
two real concessions are made: (a) the legacy crate is exempted from
the strict clippy gate, because today's code has unresolved warnings
across `chunker`, `parser`, `graph`, and `semantic`; (b) the toolchain
flip from `nightly` (current `rust-toolchain.toml`) to stable `1.95.0`
is verified before Phase 0 ships. Both are explicit steps below.

**Steps.**

1. **Verify stable build.** Before any move, run
   `cargo +1.95.0 build --bin file-search-mcp` against the current
   single-crate layout. If the legacy code uses any nightly-only feature
   (it shouldn't — repo is on nightly by historical accident), patch
   those out FIRST. Phase 0 cannot proceed until the existing code
   builds on stable 1.95.0.
2. Move the existing crate to `crates/file-search-mcp-legacy/`. It
   stays functionally unchanged; only its location and binary target
   name change.
3. Write the virtual `Cargo.toml` at the repo root with `resolver = "3"`,
   `[workspace.package]` (edition `2024`, MSRV `1.95`),
   `[workspace.dependencies]` (mirror current deps), and
   `[workspace.lints]` (rust + clippy).
4. **Exempt legacy from strict lints.** Add a package-level `[lints]`
   table in `crates/file-search-mcp-legacy/Cargo.toml` that overrides
   the workspace lints to warn-only (NOT deny):
   `[lints.rust] missing_docs = "allow"`, etc. New crates inherit the
   strict workspace lints. A "legacy lint cleanup" sub-task runs in
   parallel with Phase 1; the exemption is removed when complete (gate
   for Phase 8 decommission).
5. Pin `rust-toolchain.toml` to `1.95.0` with components
   `rustfmt, clippy, rust-src`. Sync the `flake.nix` rust attribute in
   the same commit.
6. Create eight empty placeholder crates under `crates/` (the seven
   target crates + `xtask`). Each has a minimal `Cargo.toml`, a `lib.rs`
   with crate-root docs and per-crate `#![warn(...)]` attributes, and
   nothing else.
7. Write `deny.toml` (advisories from RustSec, license allow-list,
   duplicate-crate detection, ban list).
8. Implement two `xtask` subcommands:
   - `xtask forbidden-deps` — parses `cargo metadata` and rejects
     forbidden edges; xtask itself is excluded.
   - `xtask smoke` — runs the smoke checklist (§7) against
     `fixtures/sample-workspace/`. Phase 0 ships with the subcommand
     working against the legacy binary; subsequent phases keep it
     green.
9. Create `fixtures/sample-workspace/` — a small (~5-file) Rust
   crate with known symbols, imports, and call relationships. Used by
   `xtask smoke` and by integration tests in every phase.
10. Wire CI: `cargo build --workspace --locked`,
    `cargo clippy --workspace --lib --bins -- -D warnings`
    (legacy is warn-only via its `[lints]`; new crates are strict),
    `cargo deny check`, `cargo run -p xtask -- forbidden-deps`,
    `cargo run -p xtask -- smoke`, `cargo public-api` baseline on each
    placeholder.
11. Add `architecture.md` at the repo root noting the migration is in
    progress; link to this plan and to `.docs/workspace-plan/`.

**Acceptance gate.**
- [ ] `cargo +1.95.0 build` succeeded against the original single-crate
      layout BEFORE the workspace move.
- [ ] All five CI gate commands green: build, clippy, deny, forbidden-deps,
      smoke.
- [ ] `xtask smoke` exits 0 against `fixtures/sample-workspace/` (against
      the legacy binary at this phase).
- [ ] `cargo public-api -p rcm-paths` (and other strict-tier placeholders)
      reports an empty public surface — establishes the baseline.
- [ ] `rust-toolchain.toml` pinned at `1.95.0`; `flake.nix` synced.
- [ ] Legacy `[lints]` exemption documented in `crates/file-search-mcp-legacy/Cargo.toml`
      with a `# REMOVED-IN-PHASE-8` comment.

**Rollback.** `git revert` the workspace manifest commit; legacy crate
untouched.

**Risk.** Low-medium. The toolchain flip and the legacy lint exemption
are real concessions (not "shell only"). The `--all-targets` clippy gate
is intentionally narrow to avoid blocking on the stale `examples/`/`benches/`
already in the
repo; widening to `--all-targets` is a Phase 1 acceptance criterion
after example cleanup.

### Phase 1 — Facade APIs over legacy

**Goal.** The eight target crates exist with their full public APIs as
adapters delegating to the unchanged legacy crate. The binary depends
only on the new crates. **No behavior changes.**

**Steps.**

1. **Bottom-up implementation order.** `rcm-paths` first. The legacy
   raw-path-string hash recipe is moved out of legacy and into the leaf
   **without modification** — `rcm-paths::resolve` produces the same
   `<dir_hash>` strings today's code produces, so existing on-disk
   indexes are not orphaned. If the three legacy hash sites
   (`tools/project_paths.rs`, `graph/ids.rs`, `indexing/incremental.rs`)
   currently disagree, `rcm-paths` exposes the per-consumer functions
   they each used; unifying them is a Phase 7 decision, not Phase 1.
   Legacy now depends on `rcm-paths` — the only allowed Phase-1 back-edge.
   Then `rcm-ra-syntax` (re-exports). Then `rcm-ra-host` (wraps legacy's
   two `load` paths). Then `rcm-embedding` (wraps legacy's
   `EmbeddingGenerator` behind the sealed `Embed` trait). Then capability
   crates: `rcm-search`, `rcm-graph`, `rcm-ide`. Finally `rcm-server`
   (the binary) replaces legacy's `main.rs`.
2. **Adapter modules.** Each capability crate has a `pub(crate) mod
   legacy_adapter` containing `From`/`TryFrom` between capability DTOs
   and legacy types. No bare `fn convert` helpers.
3. **Legacy as private dep.** Capability crates declare
   `file-search-mcp-legacy = { path = "../file-search-mcp-legacy" }`.
   Legacy is private — capability crates do not re-export legacy types.
4. **Forbidden-deps tightens.** The script now enforces
   capability-to-capability forbidden edges; everyone may transitively
   pull legacy.
5. **Tool routing.** Each MCP tool's handler in `rcm-server` delegates
   to the right capability service via the `*Params` → request DTO →
   service method → response DTO → `Content::text` path.
6. **Retire stale legacy `examples/`/`benches/`.** With the binary now
   in `rcm-server`, several legacy examples are obsolete. Delete or
   move them under a `legacy/examples/_retired/` directory; widen the
   clippy gate to `--all-targets` once they're gone.

**Acceptance gate.**
- [ ] All MCP tools pass the smoke checklist against the new binary.
- [ ] `cargo public-api -p rcm-search` (and the other capability crates)
      shows only the documented facade items.
- [ ] forbidden-deps script accepts `legacy` as a transitive dep but
      rejects capability-to-capability edges.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
      (after example retirement).

**Rollback.** `git revert` the binary's dep-change commit; binary
points back at legacy directly.

**Risk.** Medium (largest surface area in the plan). Mitigated by being
adapter-only — no algorithm moves, no storage changes, no flag flips.

### Phase 2 — Hidden singleton removal

**Goal.** Delete every `LazyLock<Mutex<…>>` runtime static.

**Steps.**

1. Audit current statics; the known case is
   `static SEMANTIC: LazyLock<Mutex<SemanticService>>` in legacy's
   `semantic` module.
2. Replace with `IdeService::open(paths)` constructed once in
   `rcm-server::main`. The cache (`HashMap<PathBuf, Arc<RaHost>>`) becomes
   a private field on `IdeService`, not a global.
3. Centralize `Embedder` construction in `main`; all sites take an
   `Arc<dyn Embed>` parameter rather than constructing their own.
4. Add a CI grep test:
   `! grep -rn 'LazyLock\|lazy_static\|^static .*Mutex' crates/`.

**Acceptance.** Grep test green; smoke checklist passes; SIGINT exit
remains clean.

**Rollback.** Single revert of the `IdeService::open` wiring.

**Risk.** Low. Mechanically straightforward.

### Phase 3 — Operation-scoped errors

**Goal.** Replace crate-level god-enums with operation-scoped
`thiserror` enums; confine `anyhow` to `rcm-server`.

**Steps.**

1. Inventory current error types in legacy.
2. Define new enums per crate (`SearchError`, `IndexError`, `CorpusError`,
   `BuildError`, `QueryError`, `AuditError`, `IdeError`, `EmbedError`,
   `PathError`, `RaError`). Public variants carry strings + stable
   component labels. Internal adapter errors are `pub(crate)` and hidden
   behind a single `Internal(InternalError)` public variant when
   needed.
3. Preserve source chains via `#[from]`.
4. Remove `anyhow` from capability `Cargo.toml` files; CI grep rejects
   future additions.
5. Audit `Drop::drop` impls; ensure they log-then-swallow, never panic.

**Acceptance.** Each capability crate has scoped enums; no god-enum
exists; `anyhow` appears only in `rcm-server` and `xtask`.

**Rollback.** Revert per crate; type changes are local.

**Risk.** Low.

### Phase 4 — Service lifetime + `ArcSwap` invalidation

**Goal.** Long-lived services constructed in `main`; per-call store
opening removed; `ArcSwap`-based invalidation; `CancellationToken`-driven
shutdown.

**Steps.**

1. Inventory per-call store opens in legacy tool handlers (LanceDB,
   Tantivy reader, `OpenedSnapshot`).
2. Replace each with an `ArcSwap`-managed handle on the relevant
   service. Writers stay behind `Mutex`.
3. Implement `SearchService::invalidate(workspace)` /
   `reload(workspace)`, `GraphService::invalidate(workspace)` /
   `reload(workspace)`, `IdeService::evict(workspace)`. `invalidate`
   drops handles; `reload` opens new handles + swaps + drops old after
   grace period. `clear_cache` calls `invalidate` (NOT `reload`).
4. Wire `clear_cache` per §3.1: refuse on `IndexBusy` → `rm -rf` scope
   paths → `invalidate`/`evict`. After clear, `search` lazily rebuilds
   (existing `UnifiedIndexer::ensure_indexed` path). Graph queries
   continue to error with "call `build_hypergraph` first" — Phase 4 does
   NOT add graph auto-rebuild; the user must explicitly re-run
   `build_hypergraph` after clearing graph data. Document this asymmetry
   in the `clear_cache` tool description.
5. `SyncManager::reload(workspace)` is the only eager-reload path,
   triggered after schema-version bumps detected by the worker.
6. Wire `CancellationToken` shutdown: `tokio::select!` between sync tick
   and cancel; on SIGINT or stdin EOF, cancel → drain in-flight tools
   (30s budget) → drop in topological order: `server` → capability
   crates → leaves.

**Acceptance.**
- [ ] `clear_cache(workspace)` followed by `search` succeeds (lazy
      rebuild); concurrent `clear_cache` during indexing returns
      `IndexBusy`.
- [ ] Manual SIGINT mid-`build_hypergraph` exits cleanly within 30s.
- [ ] No per-call `VectorStore::open` / `Bm25Search::new` /
      `OpenedSnapshot::open` remain in tool handlers.

**Rollback.** Each service's `invalidate` / `reload` is independently
revertible; per-call opens can be restored.

**Risk.** Medium. The `ArcSwap` reload-under-load path needs a bench:
the legitimate `SyncManager::reload` may briefly contend with
concurrent searches. Drop-after-grace mitigates. Lazy-rebuild latency
on first search after `clear_cache` is the user-facing cost; documented
as expected.

### Phase 5 — Sealed `Embed` trait + feature gate

**Goal.** Replace direct `legacy::EmbeddingGenerator` usage with the
sealed `Embed` trait. `rcm-graph`'s embedding dep becomes optional.

**Steps.**

1. Implement `Embed` (sealed), `Embedder = Arc<dyn Embed>`,
   `FastEmbedEmbedder` (production), `DeterministicEmbedder` (test).
2. Cargo wiring: `rcm-embedding` features `embeddings` (default-on,
   pulls fastembed/ort) and `test-fakes` (default-off, deterministic
   impl). Capability crates depend on `rcm-embedding` with
   `default-features = false`. Binary enables `embeddings`.
3. `rcm-graph`'s `rcm-embedding` dep is itself optional behind feature
   `semantic-overlaps` (default-off in `rcm-graph`'s manifest; default-on
   in `rcm-server`'s dep on `rcm-graph`). `GraphService::semantic_overlaps`
   returns `EmbedderUnavailable` when no embedder is configured.
4. Add a compile-time `Send + Sync` assertion on `TextEmbedding` inside
   `rcm-embedding` so an upstream change fails the build (contingency:
   single-thread worker pattern).
5. Migrate existing tests to `DeterministicEmbedder` via `test-fakes` in
   `[dev-dependencies]`. No real ONNX load in unit tests.

**Acceptance.**
- [ ] `cargo tree -p rcm-search | grep -E '(fastembed|ort)'` is empty.
- [ ] `cargo build -p rcm-graph --no-default-features` succeeds with
      no fastembed/ort in the dep tree.
- [ ] `semantic_overlaps` and `similar_to_item` work in the full binary.

**Rollback.** Feature flags can be flipped back; the trait abstraction
itself is revertible by inlining `FastEmbedEmbedder`.

**Risk.** Low.

### Phase 6 — Parser scope reduction

**Goal.** The chunker uses only `rcm-ra-syntax` for chunking-context
extraction (last-segment call names, raw `use` paths, file-scoped symbol
kinds). Structural tools (`get_dependencies`, `get_call_graph`,
`analyze_complexity`) move to `rcm-graph` and route through HIR.

**This is the highest-risk phase.** The cold path of `index_codebase`
on a fresh workspace may now require a HIR snapshot before chunking can
extract resolved structure. A measurement gate must pass before merge.

**Steps.**

1. Audit current parser usage; categorize each call site
   (chunker-context, ingestion-metadata, structural-tool).
2. Define the chunker's reduced extractor in `rcm-search`. It produces
   only embedding-context fields, not resolved structure. **The
   chunker's signature carries an inert `Option<&OpenedSnapshot>`
   parameter from day one** even though the Phase-6 implementation
   ignores it. This freezes the public chunker API for the future case
   where snapshot-resolved chunking context is wanted; later phases
   start using it without an API break. The `Option` is the simplest
   forward-compatible shape and costs nothing to thread.
3. Decide Tantivy field policy: for each field today populated by
   parser-derived structure, choose drop / approximate-via-ra-syntax /
   move-to-HIR-resolved. Recommend keeping fields populated by
   approximations (no schema change in Phase 6); schema changes belong
   in Phase 7.
4. Move `get_dependencies`, `get_call_graph`, `analyze_complexity` to
   `rcm-graph`. They read from `OpenedSnapshot`. (`analyze_complexity`
   may keep an AST walk via `rcm-ra-syntax` since complexity isn't in
   the snapshot model — graph already does AST-driven audits.)
5. Update `rcm-server` tool dispatch to call `GraphService` for these
   three tools.
6. Delete the now-unused legacy parser modules (`call_graph.rs`,
   `imports.rs`, `type_references.rs`). Keep chunking-context
   functions only.
7. **Cold-start measurement gate.** Bench `index_codebase` cold against
   a fixture workspace before vs. after Phase 6. If the after time is
   >2× the before time, gate the change behind an opt-out flag
   (`--prebuilt-snapshot=false` falls back to the parser-only chunker).

**Acceptance.**
- [ ] Chunker uses only `rcm-ra-syntax` for context.
- [ ] `get_dependencies`/`get_call_graph` results are HIR-resolved
      (strictly more accurate than the legacy parser-based versions).
- [ ] Cold-start regression documented; within 2× or behind opt-out.
- [ ] Smoke checklist passes.

**Rollback.** Partially reversible: deleted modules can be restored
from git, but if Tantivy schema changed, an index rebuild is required.
Phase 6 should NOT change the schema; that defers to Phase 7.

**Risk.** High. The cold-start ordering change is the substantive risk;
the measurement gate is mandatory.

### Phase 7 — Storage layout v2 (optional)

**Goal.** Migrate on-disk layout from v1 (per-backend top-level
subdirs) to v2 (per-workspace top-level partition with per-area
versioned subdirs). Operationally the riskiest phase; deferrable
indefinitely.

**Steps.**

1. Implement `xtask migrate-storage` with `--dry-run` (default to print
   plan), `--resume` (continue interrupted run), and an explicit
   `--from-version` / `--to-version` range.
2. Migration algorithm: acquire lock file → check current
   `LAYOUT_VERSION` → compute moves as a list → execute via atomic
   `rename` (cross-filesystem fallback to copy + verify + delete) →
   write new `LAYOUT_VERSION` last → release lock.
3. Idempotent and resumable: each move skips if the target exists;
   interruption leaves both source and target in place.
4. Backwards-compat window of one release: `rcm-paths::resolve` checks
   v2 first, falls back to v1. `clear_cache` deletes both if present.
5. Update `rcm-paths` field names to v2 vocabulary (`keyword_path`,
   `vector_path`, `metadata_path`, `merkle_path`, `graph_path`).
6. Per-area `VERSION` files: each backend reads its own version on open
   and refuses to open on mismatch, prompting a per-area
   `clear_cache(scope=…)` rebuild.
7. Per-workspace `manifest.json` records canonical workspace path,
   timestamps, backend metadata. Self-describes the hash directory.
8. Expand `clear_cache` to accept
   `scope: Workspace | Keyword | Vector | Metadata | Merkle | Graph |
   All`. Default stays `Workspace` for compat. Tool refuses paths
   outside `<XDG-data>/rust-code-mcp/workspaces/` as a safety check.
9. After one release cycle, drop v1 fallback support.

**Acceptance.**
- [ ] `xtask migrate-storage --dry-run` produces a complete plan
      against a real v1 layout.
- [ ] Real migration is read-resumable from any kill point.
- [ ] All MCP tools work against both layouts during the compat window.

**Rollback.** Migration creates new paths but does not delete old ones
until the move is verified. Reverting requires reverting
`LAYOUT_VERSION` and restarting on the old binary; new paths remain as
orphans (cleanup via a future `xtask gc-storage` or manually).

**Risk.** High operational risk (data on user disks). Low correctness
risk (idempotent + resumable + dry-run-able). Defer indefinitely until
a backend schema bump or a user complaint about `clear_cache` coarseness
forces it.

### Phase 8 — Decommission legacy

**Goal.** Delete `file-search-mcp-legacy`, all `legacy_adapter`
modules, and the `legacy → rcm-paths` Phase-1 back-edge.

**Steps.**

1. Verify `cargo tree --workspace | grep file-search-mcp-legacy` is
   empty (no remaining transitive dep).
2. Delete `crates/file-search-mcp-legacy/`.
3. Delete every `pub(crate) mod legacy_adapter` and the `From` impls
   it contained.
4. Remove the legacy entry from `[workspace.members]` and
   `[workspace.dependencies]`.
5. Tighten the forbidden-deps script to forbid any reference to
   `file-search-mcp-legacy`.

**Acceptance.**
- [ ] `cargo build --workspace` green.
- [ ] Smoke checklist passes.
- [ ] `cargo tree` shows no legacy crate.

**Rollback.** Restore the legacy crate from git. (Trivial; the deletion
is the last step.)

**Risk.** Low. By construction this phase only runs after every
capability crate's adapter has stopped delegating.

---

## 5. Phase ordering and parallelism

```
Phase 0          (must come first)
   │
   ▼
Phase 1          (largest single change)
   │
   ▼
Phase 2 ‖ Phase 3   (parallelizable; independent)
   │     │
   └──┬──┘
      ▼
Phase 4 ‖ Phase 5   (parallelizable; touch different crates)
   │     │
   └──┬──┘
      ▼
Phase 6          (highest-risk; gated by measurement)
   │
   ▼
Phase 8          (decommission)

Phase 7 is independent; can land any time after Phase 4.
```

**Estimated total effort** (single engineer, normal pace): best case 4
weeks, realistic 7 weeks, worst case 12 weeks. Phases 2/3 and 4/5 each
save 1–2 weeks if parallelized across two engineers.

---

## 6. Risk register

| ID | Risk | Phase | Mitigation |
|---|---|---|---|
| R1 | HIR-backed structural tools regress cold-start `index_codebase` >2× | 6 | Measurement gate; opt-out flag if exceeded |
| R2 | Lazy rebuild after `clear_cache` mistaken for a hang | 4 | Document as expected; consider opportunistic pre-warm on `track_directory` |
| R2b | `ArcSwap` swap contention on legitimate `SyncManager::reload` | 4 | Swap-then-drop-after-grace; bench concurrent read+reload |
| R3 | Storage v2 migration loses data on interrupt | 7 | `--dry-run` mandatory; `LAYOUT_VERSION` written last; v1 paths retained until v2 read-back verified |
| R4 | Phase 1 adapter conversions dominate ingestion time | 1 | `From`/`TryFrom` over moves not clones; bench end-to-end |
| R5 | forbidden-deps script too strict, blocks legitimate refactors | 0 | Policy in version control; PR-adjustable with review |
| R6 | Sealed `Embed` cannot fit a future test scenario | 5 | Add a third sealed impl in `rcm-embedding` rather than unsealing |
| R7 | `RaHost::with_db` closure form awkward in graph hot path | 1, 6 | Typed views (`local_crates`, `vfs`) for common cases; only fall through to closure for true HIR walks |
| R8 | `#[allow(clippy::disallowed_methods)]` bypassed by accidental refactor outside graph/ide | ongoing | CI grep blocks; review enforces |
| R9 | Tantivy schema field drops in Phase 6 force index rebuild | 6 | Keep fields populated by ra-syntax approximations in Phase 6; defer schema changes to Phase 7 |
| R10 | Phase 8 decommission misses a stray `legacy` use | 8 | `cargo tree` check; tighten forbidden-deps script before deletion |

---

## 7. Smoke checklist

After every phase, the following MCP tool calls must succeed against a
fixture workspace:

- `index_codebase`
- `search` with a known keyword
- `find_definition` on a known symbol
- `find_references` on a known symbol
- `build_hypergraph`
- `who_calls`, `who_imports`, `workspace_stats`
- `get_dependencies`, `get_call_graph`, `analyze_complexity`
- `semantic_overlaps` (when `embeddings` feature is on)
- `similar_to_item`
- `clear_cache(workspace)` followed by `search` (verifies on-disk
  delete + handle invalidation + lazy rebuild on next read)

The `xtask smoke` subcommand exercises the checklist against
`fixtures/sample-workspace`.

---

## 8. Decisions to make before starting

These should not be deferred past Phase 0:

1. **Crate naming.** Keep `rcm-*` prefix or pick another (`code-*`,
   unprefixed). Recoverable in Phase 0 only; baked into manifests
   afterward.
2. **Legacy disposition.** Adapter shim through Phases 1–7 (this plan
   assumes this), or absorb directly?
3. **Tantivy schema policy in Phase 6.** Tolerate cold-start regression
   for full HIR resolution, or keep ra-syntax approximations?
4. **Phase 7 trigger.** Ship now, or defer until a specific schema
   change motivates it?
5. **SDK consumer roadmap.** If external Rust crates are expected to
   depend on capability crates, public API discipline tightens further
   (more `#[non_exhaustive]`, formal MSRV promise, semver guarantees).

---

## 9. Out of scope

Explicitly NOT in this plan:

- Adding a second embedding provider.
- Adding a non-stdio MCP transport.
- Adding an HTTP server frontend.
- Replacing LanceDB / Tantivy / fastembed.
- Multi-tenancy.
- Changing the workspace-hash recipe (frozen as
  `sha256(canonicalize(workspace).as_encoded_bytes())`, lower-hex).

These are not part of this migration. If they happen later, they get
their own plans.
