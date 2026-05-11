# rust-code-mcp v3 — Workspace Refactor Plan

Status: draft, derived from a 50-agent audit of `rust-code-mcp-final` (monolith),
`rust-code-workspace` (v1, 5 crates), `rust-code-2` (v2, 8 crates + xtask),
and the frozen baseline in `.docs/workspace-plan/DECISIONS.md`.

Starting point: this repository (`rust-code-mcp-final`), the single-crate monolith
`file-search-mcp` at HEAD. Target: a 9-crate workspace + `xtask`, branded
`rust-code-mcp-*`, single binary `rust-code-mcp`.


## 1. Goal

Convert the monolith into a capability-keyed workspace where the type system
enforces the architecture invariants the prior two refactors tried to enforce
by convention. Every phase compiles. No phase introduces a "legacy" private
dep that survives past its own phase. The plan is executed in-place on this
repository; no parallel "v3" tree.

### Non-goals (explicit)

- No on-disk storage layout change. Existing `~/.local/share/.../{merkle,graph,vectors}`
  paths keep working. Storage v2 is out of scope.
- No new MCP tools. The ~50 tools in `TOOLS.md` keep identical names and schemas.
- No tantivy/lancedb/heed/ra_ap_* version bumps as part of the refactor.
- No `core` / `common` / `shared` / `model` / `domain-types` catch-all crate.
  v1 and v2 both wrote it down as anti-goal; we honor it.
- No async churn in domain cores. Async lives at service-method boundaries only.
- No global `LazyLock<Mutex<T>>` singletons (kept or introduced).


## 2. Target architecture

### 2.1 Dependency graph (no cycles, no cross-capability edges)

```
                                ┌──────────────────────────────┐
                                │  rust-code-mcp-server        │ ← bin "rust-code-mcp"
                                │  (rmcp, tokio, anyhow,       │   ALL #[tool] live here
                                │   Config, SyncManager)       │
                                └──┬────┬────┬────┬────┬───────┘
        ┌──────────────────────────┘    │    │    │    │
        ▼                               ▼    ▼    ▼    ▼
  ┌──────────────────┐    ┌──────────────────┐  ┌──────────────────┐
  │ ...-search       │    │ ...-graph        │  │ ...-ide          │
  │ (tantivy,lance,  │    │ (heed)           │  │ (thin facade)    │
  │  sled,arrow)     │    │                  │  │                  │
  └──────┬───────────┘    └───────┬──────────┘  └──────┬───────────┘
         │  forbidden: ─── X ─── X ─── X ───  forbidden│
         ├──────┬─────────────────┼────────────────────┤
         │      │                 │                    │
         ▼      ▼                 ▼                    ▼
  ┌──────────┐ ┌──────────────┐  ┌──────────────────────┐
  │ ...-parse│ │ ...-embedding│  │ ...-ra-host          │
  │(ra_ap_   │ │(ort,fastemb) │  │(ra_ap_ide/ide_db/    │
  │ syntax)  │ │              │  │ hir/hir_def/         │
  │          │ │              │  │ load-cargo/vfs/      │
  │          │ │              │  │ project_model/       │
  │          │ │              │  │ base_db)             │
  └────┬─────┘ └──────┬───────┘  └──────┬────────────────┘
       │              │                  │
       └──────┬───────┴──────────────────┘
              ▼
  ┌──────────────────┐
  │ ...-paths        │ ← leaf (directories, sha2)
  └──────────────────┘

  rust-code-mcp-search-eval  ← orphan, depends only on -search public DTOs
  xtask                       ← orthogonal; zero workspace edges
```

### 2.2 Crate roster

| # | Crate | Owns | Heavy deps | Consumers |
|---|---|---|---|---|
| 1 | `rust-code-mcp-paths` | path/hash conventions, dir layout v1 | `directories`, `sha2` | search, graph, ide, server |
| 2 | `rust-code-mcp-parse` | `ra_ap_syntax` wrapping: `SyntaxLayer`, `Import`, `CallEdge`, `Symbol` extraction | `ra_ap_syntax` | search, ide |
| 3 | `rust-code-mcp-embedding` | sealed `Embedder` trait, `EmbeddingRuntime`, `FakeEmbedder` (test-support feature) | `fastembed`, `ort`(+CUDA) | search, graph, server |
| 4 | `rust-code-mcp-ra-host` | `WorkspaceHandle`, `with_db`/`with_semantics` closures, actor-owned `AnalysisHost` | all heavy `ra_ap_*` | graph, ide |
| 5 | `rust-code-mcp-search` | `SearchService`, `CorpusWriter`, hybrid retrieval, ingest pipeline, chunker, secrets-filter, tantivy schema, vector store | `tantivy`, `lancedb`, `sled`, `arrow_*`, `rs_merkle` | server |
| 6 | `rust-code-mcp-graph` | HIR-driven hypergraph, queries split into `queries/{calls,crates,overlaps,reexports,modules,attributes,metrics}.rs`, audits split into `audits/*.rs` | `heed` | server |
| 7 | `rust-code-mcp-ide` | `IdeService`: find_definition, find_references, file-scoped get_dependencies/get_call_graph/analyze_complexity | none direct (via -ra-host) | server |
| 8 | `rust-code-mcp-server` | `main.rs` (bin `rust-code-mcp`), `lib.rs`, `tools/{search,graph,ide,audits}/*.rs`, `Config`, `SyncManager`, `monitoring/`, `metrics/`, rmcp bootstrap, graceful shutdown | `rmcp`, `tokio`, `anyhow` | (none — root) |
| 9 | `rust-code-mcp-search-eval` | offline IR metrics (NDCG/MRR/MAP), `RrfTuner`, `RankedSearch` trait | none (pure) | (none — orphan) |
| – | `xtask` | dev/CI dispatcher: smoke, policy, baseline, RPC harness | `syn`, `quote`, `toml`, `anyhow` | (none) |

### 2.3 Binary inventory

- `rust-code-mcp` from `crates/rust-code-mcp-server/src/main.rs` — the only production binary.
- `xtask` from `xtask/src/main.rs` — dev only.
- No `[[example]]` declarations. No `examples/` directory. Surviving diagnostics from the
  monolith's `examples/` (21 files) are either deleted, converted to integration tests
  under a single crate's `tests/`, or moved to `xtask` subcommands.


## 3. Hard rules (enforced via tests + lints, not convention)

1. **No SDK types in capability public APIs.** `rmcp`, `heed`, `tantivy`, `lancedb`,
   `arrow_*`, `ra_ap_ide`, `ra_ap_hir*`, `ra_ap_ide_db`, `ra_ap_load-cargo`,
   `ra_ap_project_model`, `ra_ap_vfs`, `ra_ap_base_db`, `sled`, `fastembed`, `ort`
   must NOT appear in a `pub fn` / `pub struct` / `pub type` signature of any of
   `-search`, `-graph`, `-ide`. Exempt — and *only* because their job is to expose
   a controlled subset:
   - `-parse` exposes a hand-picked subset of `ra_ap_syntax`
   - `-ra-host` exposes a controlled subset of `ra_ap_ide_db` via closures
   - `-embedding` exposes its sealed `Embedder` trait
2. **No cross-capability dependencies.** `search ⊥ graph ⊥ ide`. Mixed-capability
   tools (`similar_to_item`, `semantic_overlaps`) compose in `-server`, never via
   direct deps. Enforced by `crates/rust-code-mcp-server/tests/architecture_boundaries.rs`.
3. **All `#[tool]` handlers live in `-server`.** Domain crates expose typed
   `*Request` / `*Output` structs + a `*Service` method per tool. No `tool_bridge`,
   no `rmcp::CallToolResult` round-trips, no duplicated `*Params` structs.
4. **No global `LazyLock<Mutex<T>>` singletons.** `AnalysisHost` lives behind a
   `std::thread` actor in `-ra-host` exposing `async fn` over `tokio::sync::oneshot`.
   The embedding runtime is `Arc<dyn Embedder>` constructed once in `main` and
   threaded through `AppServices`. Both are owned by `AppServices`, dropped in `main`.
5. **`pub(crate)` by default.** Each crate's `lib.rs` is its entire public surface.
   Submodules are `pub(crate)` unless explicitly re-exported from `lib.rs`. A
   `pub use crate::foo::Bar` in `lib.rs` is the only way a type leaves the crate.


## 4. Naming policy

- Workspace crates: `rust-code-mcp-<suffix>` (e.g. `rust-code-mcp-server`).
- Binary: `rust-code-mcp` (set via `[[bin]] name = "rust-code-mcp"` in server crate).
- Package name of the server crate: `rust-code-mcp-server` so
  `Implementation::from_build_env()` reports `rust-code-mcp-server` cleanly.
- The string `file-search-mcp` is removed everywhere except git history.
- `rcm-*` is not used; v2's short prefix is opaque and unsearchable.
- `.mcp.toml` server label stays `rust-code-mcp-new` / `-old` (user-chosen at site).


## 5. Validation discipline (every phase)

Every phase ends with all of:

```
nix develop ../nix-devshells#code --command cargo check --workspace --all-targets
nix develop ../nix-devshells#code --command cargo clippy --workspace --all-targets -- -D warnings
nix develop ../nix-devshells#code --command cargo run -p xtask -- baseline
```

Do not run `cargo test` between phases unless the phase touches test code; the
snapshot fixture costs ~115 s. Use `cargo check --lib -p <crate>` for tight
inner-loop verification during a phase.

Each phase has its own `Verify:` block below — those are the minimum that must
pass; the workspace-wide checks above are the always-required gate.

Each phase is one git/jj commit. If a phase fails halfway, revert and restart;
do not paper over with intermediate fixups inside the phase.


## 6. Risk register (forward look)

| Risk | Likelihood | Phase | Mitigation |
|---|---|---|---|
| `-ra-host` actor pattern hits salsa thread-affinity issue | medium | 4 | Start with sync API + `spawn_blocking` at server; promote to actor only if measurements demand it. |
| `-graph::queries.rs` (3,571 LOC) split introduces test regressions | medium | 6 | Move file unchanged first; split files in a follow-up commit within the same phase; tests run after each file split. |
| `compute_fingerprint` and `FileSystemMerkle` accidentally diverge during split | low | 5+6 | Keep them in `-search` and `-graph` respectively (they always were independent); add a property test that both walk the same file set. |
| `tool_bridge.rs` deletion breaks a tool we missed | low | 8 | The bridge round-trips text DTOs; deleting it removes a parsing site that already had unit tests. Use `--all-targets` after deletion. |
| Cargo workspace pulls `ra_ap_*` into every crate via feature unification | medium | 4 | Verify per-crate `cargo build -p <crate>` does not transitively pull `ra_ap_ide` into `-search`. Use `cargo tree -p <crate> -e normal --depth 2` after every dep change. |
| Renaming `file-search-mcp` → `rust-code-mcp-server` breaks downstream `.mcp.toml` integration | low | 0 | The `.mcp.toml` symlinks point to a binary path, not a package name. Binary name (`rust-code-mcp`) is set independently via `[[bin]] name = ...`. |
| Snapshot build cost slows iteration | high | 4–7 | Per-crate `cargo check --lib` everywhere; full snapshot test only at end of each phase. |


## 7. Phases

Each phase is structured:
- **Goal** — one sentence.
- **Prereqs** — phases that must be complete.
- **Steps** — numbered, concrete, file-paths.
- **Files touched** — moved / created / deleted.
- **Verify** — minimum checks beyond workspace-wide gate.
- **Rollback** — single revert command.
- **Done when** — observable end state.

---

### Phase 0 — Workspace skeleton

**Goal.** Turn this single-crate repo into a workspace whose only member is
`crates/rust-code-mcp-server`, containing the entire current `src/` tree
verbatim. No code moves yet; no behavior changes.

**Prereqs.** None.

**Steps.**

1. `mkdir -p crates/rust-code-mcp-server`.
2. `git mv src crates/rust-code-mcp-server/src`.
3. `git mv tests crates/rust-code-mcp-server/tests`.
4. `git mv benches crates/rust-code-mcp-server/benches` (if non-empty).
5. `git mv examples crates/rust-code-mcp-server/examples` (note: many will be
   deleted in Phase 10; keep them under the server crate for now so they compile).
6. Move the root `Cargo.toml` to `crates/rust-code-mcp-server/Cargo.toml`.
   Rename package: `name = "rust-code-mcp-server"`. Add binary override:
   ```toml
   [[bin]]
   name = "rust-code-mcp"
   path = "src/main.rs"
   ```
7. Write a new root `Cargo.toml`:
   ```toml
   [workspace]
   resolver = "3"
   members = ["crates/*"]

   [workspace.package]
   edition = "2024"
   version = "0.1.0"
   publish = false
   rust-version = "1.85"

   [workspace.dependencies]
   # All third-party deps hoisted here, alphabetized.
   # (Copy from old root Cargo.toml verbatim.)

   [workspace.lints.clippy]
   dbg_macro    = "warn"
   todo         = "warn"
   unwrap_used  = "warn"

   [profile.dev.package."*"]
   opt-level = 1   # faster dep builds locally
   ```
8. In `crates/rust-code-mcp-server/Cargo.toml`, convert every dep to
   `{ workspace = true }` form. Add `[lints] workspace = true`.
9. Copy v2's `deny.toml` to the workspace root. Adjust git URLs to the two we
   actually use (`Anush008/fastembed-rs`, `modelcontextprotocol/rust-sdk`).
10. Update `rust-toolchain.toml` to pin a stable channel + components:
    ```toml
    [toolchain]
    channel    = "1.85.0"
    components = ["rustfmt", "clippy", "rust-src", "rust-analyzer"]
    ```
11. Inside `crates/rust-code-mcp-server/src/lib.rs`, add a top doc block:
    ```rust
    //! rust-code-mcp server. Owns rmcp transport, tool dispatch, Config,
    //! SyncManager. Composes capability services (-search, -graph, -ide).
    //!
    //! This crate owns:
    //!   - MCP tool handlers (#[tool] surface)
    //!   - Top-level Config and CLI/server lifecycle
    //!   - Background sync, graceful shutdown
    //!
    //! Non-goals (belong to sibling crates):
    //!   - rust-analyzer integration (-ra-host)
    //!   - Text/vector search, indexing pipeline (-search)
    //!   - Persisted hypergraph and graph queries (-graph)
    //!   - IDE-style navigation (-ide)
    ```

**Files touched.** Root `Cargo.toml`, `crates/rust-code-mcp-server/**` (move).
New: `deny.toml`. Edited: `rust-toolchain.toml`.

**Verify.**
```
nix develop ../nix-devshells#code --command cargo build --bin rust-code-mcp
ls target/release/rust-code-mcp 2>/dev/null && echo "binary built"
```
Smoke: `nix develop ../nix-devshells#code --command cargo run --bin rust-code-mcp -- --help`
(or whatever the server's help mechanism is — at minimum it should start, accept
stdio, then exit cleanly on EOF).

**Rollback.** `jj abandon` / `git reset --hard HEAD~1`.

**Done when.** `cargo build --workspace` succeeds. The binary at
`target/{debug,release}/rust-code-mcp` runs identically to the pre-Phase 0
`file-search-mcp` binary. `.mcp.toml` symlinks resolve to the new path.

---

### Phase 1 — Extract `rust-code-mcp-paths` (leaf, warmup)

**Goal.** Create the first sibling crate. Validate that the move-with-imports
workflow is solid before touching anything risky.

**Prereqs.** Phase 0.

**Steps.**

1. `mkdir -p crates/rust-code-mcp-paths/{src,tests}`.
2. Create `crates/rust-code-mcp-paths/Cargo.toml`:
   ```toml
   [package]
   name        = "rust-code-mcp-paths"
   version     = { workspace = true }
   edition     = { workspace = true }
   publish     = { workspace = true }
   rust-version = { workspace = true }

   [dependencies]
   directories = { workspace = true }
   sha2        = { workspace = true }
   thiserror   = { workspace = true }

   [lints]
   workspace = true
   ```
3. Identify path/hash code in the server crate. Likely sites
   (search the server's `src/` for these patterns first):
   - `ProjectDirs::from("dev","rust-code-mcp","search")` usages
   - `directory_hash` / `workspace_hash` SHA-256 helpers
   - `GraphPaths`, `MerklePaths`, `SearchPaths`, `StoragePaths`, `ConfigPaths`
     structs (currently scattered in `graph/storage.rs`, `indexing/merkle.rs`,
     `tools/project_paths.rs`, etc.)
4. Move all of the above into `crates/rust-code-mcp-paths/src/lib.rs`. Group
   under `pub mod v1 { pub mod config; pub mod search; pub mod merkle; pub mod graph; }`
   exactly as v2's `rcm-paths` did (one-time freeze of the path schema).
5. Add `pub use v1::{config::ConfigPaths, search::SearchPaths, ...}` at crate root
   for ergonomic imports.
6. Define `pub enum PathError` (thiserror) for any errors that escape the crate.
7. In every server file that referenced the moved code, replace with
   `use rust_code_mcp_paths::{...}`. Use `cargo check -p rust-code-mcp-server`
   to drive the imports list to closure.
8. Add `rust-code-mcp-paths = { workspace = true }` to
   `[workspace.dependencies]` AND to the server's `[dependencies]`.
9. Write `crates/rust-code-mcp-paths/tests/v1_compat.rs` asserting that the
   `data_dir` / `directory_hash` outputs match a hand-rolled fixture so future
   reorganizations cannot silently change the on-disk path recipe. Hand-roll
   a tiny fixture; do not depend on the snapshot.
10. Add crate-level doc block per Hard Rule 5 template (the "What this crate
    owns / does not own" block, see Phase 0 step 11 example).

**Files touched.** New: `crates/rust-code-mcp-paths/{Cargo.toml,src/lib.rs,tests/v1_compat.rs}`.
Edited: ~5–10 server files (imports), root `Cargo.toml` (workspace dep),
server `Cargo.toml` (new dep).

**Verify.**
```
nix develop ../nix-devshells#code --command cargo check -p rust-code-mcp-paths --lib
nix develop ../nix-devshells#code --command cargo test  -p rust-code-mcp-paths --tests
nix develop ../nix-devshells#code --command cargo tree  -p rust-code-mcp-paths -e normal
```
The dep tree should be exactly `directories`, `sha2`, `thiserror` (+ transitives).

**Rollback.** Single commit revert.

**Done when.** The string `ProjectDirs::from` appears in no file under
`crates/rust-code-mcp-server/src/` — only in `crates/rust-code-mcp-paths/`.

---

### Phase 2 — Extract `rust-code-mcp-parse`

**Goal.** Pull the `ra_ap_syntax`-based parser out of the server. Set up the
syntax-only firewall before `-ra-host` claims the heavy stack.

**Prereqs.** Phases 0–1.

**Steps.**

1. `mkdir -p crates/rust-code-mcp-parse/src`.
2. `Cargo.toml`:
   ```toml
   [package]
   name        = "rust-code-mcp-parse"
   # ... workspace inheritance ...

   [dependencies]
   ra_ap_syntax = { workspace = true }
   serde        = { workspace = true }    # for serializable AST DTOs
   thiserror    = { workspace = true }
   uuid         = { workspace = true }    # ChunkId

   [lints]
   workspace = true
   ```
3. Move from server: `src/parser/{mod.rs, call_graph.rs, imports.rs, type_references.rs}` →
   `crates/rust-code-mcp-parse/src/`.
4. Public API surface (in `lib.rs`):
   ```
   pub struct SyntaxLayer; // ZST entry point
   pub fn parse_source(text: &str) -> ParsedSource
   pub fn parse_source_with_edition(text: &str, ed: Edition) -> ParsedSource
   pub fn analyze_source(text: &str) -> SourceAnalysis

   pub struct ParsedSource { ... }
   pub struct SourceAnalysis { ... }
   pub struct Symbol { ... } // unified with v1's SymbolKind
   pub struct Import { ... }
   pub struct CallEdge { ... }
   pub struct CallGraph { ... }
   pub enum ParseError { ... }
   ```
5. Move `Edition` re-export and `SyntaxNode` exposure ONLY if a consumer needs
   them. Goal: callers should be able to import `rust_code_mcp_parse::*` and
   never need to add a direct `ra_ap_syntax` dep.
6. Server's `Cargo.toml`: remove `ra_ap_syntax` from `[dependencies]`. Add
   `rust-code-mcp-parse = { workspace = true }`. Update all
   `use ra_ap_syntax::*` and `use crate::parser::*` import sites.
7. Add `rust-code-mcp-parse = { workspace = true }` to workspace deps.
8. Crate-level doc block per Rule 5.

**Files touched.** New: `crates/rust-code-mcp-parse/**`. Moved out of server:
`src/parser/*` (4 files). Edited: many server files (import migration).

**Verify.**
```
nix develop ../nix-devshells#code --command cargo check -p rust-code-mcp-parse --lib
nix develop ../nix-devshells#code --command cargo tree  -p rust-code-mcp-parse -e normal | grep -v "^[[:space:]]*$"
# Should NOT contain: ra_ap_ide, ra_ap_hir, ra_ap_ide_db, ra_ap_load-cargo, ra_ap_vfs.
```
Grep server src/ for `use ra_ap_syntax`; should be empty.

**Rollback.** Single commit revert.

**Done when.** `cargo tree -p rust-code-mcp-server -e normal --depth 1 | grep ra_ap_`
shows `ra_ap_syntax` only via the `-parse` crate, plus the still-present heavy
stack (`ra_ap_ide`, etc.) — those leave in Phase 4.

---

### Phase 3 — Extract `rust-code-mcp-embedding`

**Goal.** Isolate the most expensive deps in the workspace (`fastembed`, `ort+cuda`)
behind a sealed `Embedder` trait so future crates can take `Arc<dyn Embedder>`
without dragging ONNX runtime in.

**Prereqs.** Phases 0–1. (Phase 2 not required.)

**Steps.**

1. `mkdir -p crates/rust-code-mcp-embedding/src`.
2. `Cargo.toml`:
   ```toml
   [package]
   name = "rust-code-mcp-embedding"
   # ... workspace inheritance ...

   [features]
   default = []
   test-support = []      # gates FakeEmbedder

   [dependencies]
   anyhow     = { workspace = true }
   async-trait = { workspace = true }   # if any async methods
   fastembed  = { workspace = true }
   ort        = { workspace = true }
   thiserror  = { workspace = true }
   tracing    = { workspace = true }

   [lints]
   workspace = true
   ```
3. Move from server: `src/embeddings/{mod.rs, error.rs}` →
   `crates/rust-code-mcp-embedding/src/`.
4. Define the sealed trait:
   ```rust
   mod sealed { pub trait Sealed {} }

   pub trait Embedder: sealed::Sealed + Send + Sync + 'static {
       fn kind(&self) -> EmbedderKind;
       fn dimensions(&self) -> usize;
       fn embed(&self, text: &str) -> Result<Embedding, EmbedError>;
       fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Embedding>, EmbedError>;
   }

   pub type Embedding = Vec<f32>;
   pub const EMBEDDING_DIM: usize = 384;

   pub struct EmbeddingRuntime { /* Arc<EmbeddingGenerator>; */ }
   impl Embedder for EmbeddingRuntime { ... }
   impl sealed::Sealed for EmbeddingRuntime {}

   #[cfg(any(test, feature = "test-support"))]
   pub struct FakeEmbedder { /* deterministic */ }
   #[cfg(any(test, feature = "test-support"))]
   impl Embedder for FakeEmbedder { ... }
   #[cfg(any(test, feature = "test-support"))]
   impl sealed::Sealed for FakeEmbedder {}
   ```
5. Server's `Cargo.toml`: remove `fastembed` and `ort` from `[dependencies]`;
   add `rust-code-mcp-embedding = { workspace = true, features = ["test-support"] }`
   ONLY where tests need it. Production code uses the bare dep.
6. Replace every `EmbeddingGenerator::new_*` construction site in the server with
   `EmbeddingRuntime::new_production()`. Construct once, in `main`, store in
   `AppServices`.
7. Crate-level doc block per Rule 5. Explicitly state non-goal: "no chunking,
   no vector store, no model selection logic beyond construction-time choice."

**Files touched.** New: `crates/rust-code-mcp-embedding/**`. Moved: server
`src/embeddings/*`. Edited: server files calling `EmbeddingGenerator` (most are
in `indexing/` and `search/`).

**Verify.**
```
nix develop ../nix-devshells#code --command cargo check -p rust-code-mcp-embedding
nix develop ../nix-devshells#code --command cargo tree  -p rust-code-mcp-server -e normal --depth 2 \
  | grep -E '\b(fastembed|ort)\b'
# Both should appear only under `rust-code-mcp-embedding`, never as a direct dep of server.
```

**Rollback.** Single commit revert.

**Done when.** `cargo tree -p rust-code-mcp-server -e normal --depth 1` shows
no direct `fastembed` or `ort` line; both reach the binary only through
`rust-code-mcp-embedding`.

---

### Phase 4 — Extract `rust-code-mcp-ra-host` (the firewall)

**Goal.** Move every heavy `ra_ap_*` crate (everything except `ra_ap_syntax`,
which lives in `-parse`) behind one crate exposing closure-based access. This
is the single highest-payoff phase: it cuts the rebuild blast radius for the
slowest deps in the workspace by ~10x.

**Prereqs.** Phases 0–2.

**Steps.**

1. `mkdir -p crates/rust-code-mcp-ra-host/src`.
2. `Cargo.toml`:
   ```toml
   [package]
   name = "rust-code-mcp-ra-host"
   # ... workspace inheritance ...

   [dependencies]
   anyhow                 = { workspace = true }
   ra_ap_base_db          = { workspace = true }
   ra_ap_hir              = { workspace = true }
   ra_ap_hir_def          = { workspace = true }
   ra_ap_ide              = { workspace = true }
   ra_ap_ide_db           = { workspace = true }
   "ra_ap_load-cargo"     = { workspace = true }
   ra_ap_paths            = { workspace = true }
   ra_ap_project_model    = { workspace = true }
   ra_ap_vfs              = { workspace = true }
   rust-code-mcp-parse    = { workspace = true }
   rust-code-mcp-paths    = { workspace = true }
   thiserror              = { workspace = true }
   tokio                  = { workspace = true, features = ["sync", "rt"] }
   tracing                = { workspace = true }

   [lints]
   workspace = true
   ```
3. Move `src/graph/loader.rs` and `src/semantic/loader.rs` from the server
   into `crates/rust-code-mcp-ra-host/src/`. Reconcile into ONE loader (these
   are near-duplicates — v2 had this same duplication and didn't fix it).
4. Public API:
   ```rust
   pub struct HostConfig {
       pub no_deps: bool,
       pub sysroot: SysrootMode,    // enum { None, Discover, Explicit(PathBuf) }
       pub prefill_caches: bool,
   }
   pub enum SysrootMode { None, Discover, Explicit(PathBuf) }
   pub struct WorkspaceHandle { /* private */ }
   pub struct HostLayer { /* private; owns an actor thread */ }
   pub enum RaError { /* thiserror */ }

   impl HostLayer {
       pub fn new(cfg: HostConfig) -> Self;
       pub async fn open_workspace(&self, root: &Path) -> Result<WorkspaceHandle, RaError>;
       pub async fn with_db<R>(&self, w: &WorkspaceHandle, f: impl FnOnce(&RootDatabase) -> R + Send) -> Result<R, RaError>
           where R: Send + 'static;
       pub async fn with_semantics<R>(&self, w: &WorkspaceHandle, f: impl FnOnce(&Semantics<'_, RootDatabase>) -> R + Send) -> Result<R, RaError>
           where R: Send + 'static;
       pub async fn analysis(&self, w: &WorkspaceHandle) -> Result<Analysis, RaError>;
   }
   ```
   Concurrency: the actor owns a long-lived `AnalysisHost` on a dedicated
   `std::thread`; closures hop via `tokio::sync::oneshot`. This kills the
   `Arc<Mutex<...>>` query serialization v2 still had.
5. Pure-RA-stack helpers (`load_workspace_at` + Vfs + AnalysisHost lifecycle)
   live here as `pub(crate)`. Two presets:
   - `open_workspace_for_ide(root)` — `no_deps=true`, `sysroot=None`, prefill caches.
   - `open_workspace_for_graph(root)` — `no_deps=false`, `sysroot=Discover`.
6. Server `Cargo.toml`: remove **all** `ra_ap_ide`, `ra_ap_ide_db`,
   `ra_ap_hir`, `ra_ap_hir_def`, `ra_ap_load-cargo`, `ra_ap_project_model`,
   `ra_ap_vfs`, `ra_ap_paths`, `ra_ap_base_db`. Add `rust-code-mcp-ra-host`.
7. Update every server call site:
   - `src/graph/extract.rs` (HIR walking) → take `&RootDatabase` parameter, body
     unchanged; caller in `-server` invokes via `host.with_db(|db| extract::run(db))`.
   - `src/semantic/mod.rs` (find_def / find_refs) → take `&Semantics<'_, _>`
     parameter; caller invokes via `host.with_semantics(...)`.
8. Hard rule check: grep server `src/` for `use ra_ap_` — must be empty after
   this phase. Only `crates/rust-code-mcp-ra-host/src/` and
   `crates/rust-code-mcp-parse/src/` import `ra_ap_*` directly.

**Files touched.** New: `crates/rust-code-mcp-ra-host/**`. Moved out of server:
`src/{graph,semantic}/loader.rs`. Edited: every server file that touched
`ra_ap_*` (~15 files in `src/graph/` + `src/semantic/`).

**Verify.**
```
nix develop ../nix-devshells#code --command cargo check -p rust-code-mcp-ra-host
nix develop ../nix-devshells#code --command cargo tree  -p rust-code-mcp-server -e normal --depth 1 \
  | grep '^ra_ap_'
# Should match nothing (server has no direct ra_ap_* dep).
grep -rn 'use ra_ap_' crates/rust-code-mcp-server/src/   # expect zero matches
```

**Rollback.** Single commit revert. This phase is the riskiest; consider
splitting into two commits inside the phase: (a) create crate + move loader,
(b) flip server to consume it. Revert (b) alone if (b) regresses tests.

**Done when.** `cargo tree -p rust-code-mcp-server -e normal --depth 1` shows
no `ra_ap_*` lines. `cargo build -p rust-code-mcp-search` does not pull
`ra_ap_ide` (verify with `cargo tree -p rust-code-mcp-search -e normal | grep ra_ap_ide`
returning nothing).

---

### Phase 5 — Extract `rust-code-mcp-search`

**Goal.** Pull search + indexing + vector store + chunker + secrets-filter +
tantivy schema + metadata cache out of the server into one capability crate.
Production logic; no MCP, no rust-analyzer.

**Prereqs.** Phases 0–3 (Phase 4 not strictly required, but recommended so the
search crate doesn't transitively pull `ra_ap_ide`).

**Steps.**

1. `mkdir -p crates/rust-code-mcp-search/src`.
2. `Cargo.toml`:
   ```toml
   [package]
   name = "rust-code-mcp-search"
   # ... workspace inheritance ...

   [dependencies]
   anyhow                  = { workspace = true }
   arrow-array             = { workspace = true }
   arrow-schema            = { workspace = true }
   async-trait             = { workspace = true }
   bincode                 = { workspace = true }
   futures                 = { workspace = true }
   glob                    = { workspace = true }
   lancedb                 = { workspace = true }
   num_cpus                = { workspace = true }
   rayon                   = { workspace = true }
   regex                   = { workspace = true }
   rs_merkle               = { workspace = true }
   rust-code-mcp-embedding = { workspace = true }
   rust-code-mcp-parse     = { workspace = true }
   rust-code-mcp-paths     = { workspace = true }
   serde                   = { workspace = true }
   serde_bytes             = { workspace = true }
   serde_json              = { workspace = true }
   sha2                    = { workspace = true }
   sled                    = { workspace = true }
   sysinfo                 = { workspace = true }
   tantivy                 = { workspace = true }
   text-splitter           = { workspace = true }
   thiserror               = { workspace = true }
   tokio                   = { workspace = true, features = ["sync", "macros"] }
   tracing                 = { workspace = true }
   uuid                    = { workspace = true }
   walkdir                 = { workspace = true }

   [dev-dependencies]
   rust-code-mcp-embedding = { workspace = true, features = ["test-support"] }
   tempfile                = { workspace = true }

   [lints]
   workspace = true
   ```
3. Move from server: `src/{search,indexing,chunker,vector_store,security}/`,
   plus root files `src/schema.rs` and `src/metadata_cache.rs`. All go under
   `crates/rust-code-mcp-search/src/`.
4. Module visibility sweep: every submodule moves in as `pub(crate)`. The
   crate's `lib.rs` re-exports only:
   - `SearchService` (read-path facade)
   - `CorpusWriter` (write-path facade)
   - DTOs: `SearchRequest`, `SearchResponse`, `SearchHit`, `IndexRequest`,
     `IndexReport`, `ClearRequest`, `ClearReport`, `HealthRequest`, `HealthReport`,
     `IndexFileRequest`, `EmbeddingRequest`, `EmbeddingResponse`, `SimilarityRequest`
   - Error: `SearchError`
5. **No tantivy, lancedb, arrow, sled, or `Index` types in public signatures.**
   Verify by writing `crates/rust-code-mcp-search/tests/public_api_contract.rs`
   that greps `src/lib.rs` for forbidden type names. (Copy v1's
   `public_api_contract.rs` pattern.)
6. Drop `notify` from deps (v2 declared but never used it).
7. Server `Cargo.toml`: remove `tantivy`, `lancedb`, `arrow-array`, `arrow-schema`,
   `sled`, `rs_merkle`, `walkdir`, `glob`, `regex`, `text-splitter`, `rayon`,
   `num_cpus`, `sysinfo`. Add `rust-code-mcp-search = { workspace = true }`.
8. Server: replace direct calls into the moved modules with `SearchService` /
   `CorpusWriter` method calls. The server's `tools/` files (which are now
   thin handler shims) call these via `AppServices`.
9. Crate-level doc block per Rule 5; explicit non-goals: no graph snapshots,
   no rust-analyzer host, no MCP types.

**Files touched.** New: `crates/rust-code-mcp-search/**`. Moved: 11 server
subdirs + 2 root files. Edited: every server `tools/` file that previously
imported `crate::search::*`, `crate::indexing::*`, etc.

**Verify.**
```
nix develop ../nix-devshells#code --command cargo check -p rust-code-mcp-search
nix develop ../nix-devshells#code --command cargo test  -p rust-code-mcp-search --tests public_api_contract
nix develop ../nix-devshells#code --command cargo tree  -p rust-code-mcp-search -e normal --depth 1 \
  | grep -E '\b(ra_ap_ide|ra_ap_hir|heed|rmcp)\b'
# All four must return nothing. Search must not transitively pull RA host, heed, or rmcp.
```

**Rollback.** Single commit revert.

**Done when.** Server's `src/` contains no `tantivy`, `lancedb`, `sled`,
`walkdir`, `glob`, `regex` imports. `public_api_contract` test passes.

---

### Phase 6 — Extract `rust-code-mcp-graph`

**Goal.** Pull the HIR-driven hypergraph (heed-backed) out. Split the two
god-files (`graph/queries.rs` 3,571 LOC and `tools/graph_tools.rs` 3,589 LOC)
during the move.

**Prereqs.** Phases 0, 1, 3, 4.

**Steps.**

1. `mkdir -p crates/rust-code-mcp-graph/src/{extract,queries,audits,model}`.
2. `Cargo.toml`:
   ```toml
   [package]
   name = "rust-code-mcp-graph"
   # ... workspace inheritance ...

   [dependencies]
   anyhow                  = { workspace = true }
   bincode                 = { workspace = true }
   heed                    = { workspace = true }
   rust-code-mcp-embedding = { workspace = true }   # for similar_to_item path
   rust-code-mcp-paths     = { workspace = true }
   rust-code-mcp-ra-host   = { workspace = true }
   serde                   = { workspace = true }
   sha2                    = { workspace = true }
   thiserror               = { workspace = true }
   tokio                   = { workspace = true, features = ["sync"] }
   tracing                 = { workspace = true }
   walkdir                 = { workspace = true }   # for fingerprint walk

   [dev-dependencies]
   rust-code-mcp-embedding = { workspace = true, features = ["test-support"] }
   tempfile                = { workspace = true }

   [lints]
   workspace = true
   ```
3. Move from server: `src/graph/*` (the 22 files now without `loader.rs`,
   which moved to `-ra-host` in Phase 4).
4. Split `src/graph/queries.rs` (3,571 LOC) — IN THIS COMMIT, before the phase
   closes — into:
   ```
   crates/rust-code-mcp-graph/src/queries/mod.rs       (shared DTOs)
   crates/rust-code-mcp-graph/src/queries/calls.rs     (who_calls, calls_from)
   crates/rust-code-mcp-graph/src/queries/crates.rs    (crate_edges, crate_metric)
   crates/rust-code-mcp-graph/src/queries/overlaps.rs  (overlaps)
   crates/rust-code-mcp-graph/src/queries/reexports.rs (re_export_chain, etc.)
   crates/rust-code-mcp-graph/src/queries/modules.rs   (module_tree)
   crates/rust-code-mcp-graph/src/queries/attributes.rs (item_attributes, etc.)
   crates/rust-code-mcp-graph/src/queries/metrics.rs   (workspace_stats)
   ```
   v1's `rust-code-mcp-graph/src/queries/` already did this split; copy that
   structure exactly.
5. Split `src/graph/*_audit.rs` (each is 400–800 LOC, already file-per-audit)
   into a `crates/rust-code-mcp-graph/src/audits/` directory:
   ```
   audits/{channel,derive,docs,fn_body,mut_static,recursion,unsafe}.rs
   ```
6. Split `src/graph/model.rs` (462 LOC) into:
   ```
   model/mod.rs
   model/{nodes,bindings,signatures,statics,usages}.rs
   ```
7. Public API (lib.rs):
   ```rust
   pub use extract::{extract, ExtractionModel};
   pub use snapshot::{OpenedSnapshot, build_and_persist, open_current};
   pub use model::{Node, NodeKind, ItemKind, Binding, Usage, ... };
   pub use queries::dto::*;          // all DTOs returned by queries
   pub use audits::dto::*;           // all DTOs returned by audits
   pub use graph_service::GraphService;
   pub use error::{GraphError, AuditError};

   // ALL the *Request and *Output structs that today live duplicated in
   // rcm-graph::tools::search_tool AND rcm-server end up here, once.
   pub struct BuildHypergraphRequest { ... }
   pub struct BuildHypergraphOutput  { ... }
   pub struct WhoCallsRequest        { ... }
   pub struct WhoCallsOutput         { ... }
   // ... etc, ~40 pairs
   ```
8. **No** `tools/` subdirectory in this crate. **No** `tool_bridge.rs`.
   **No** `rmcp` dep. Anything related to `CallToolResult` stays in the server.
9. `heed::{RoTxn, Database, Env}` must not appear in any `pub` signature.
   Wrap with crate-private `GraphRoTxn` newtypes if needed.
10. Storage layer (`src/storage.rs`, `src/snapshot.rs`) becomes `pub(crate)`;
    expose only the typed Service API.
11. Server `Cargo.toml`: remove `heed`. Add `rust-code-mcp-graph`.
12. Server: replace `use crate::graph::*` with `use rust_code_mcp_graph::*`.
    Tool handlers in `src/tools/graph_tools.rs` now call
    `services.graph.build_hypergraph(BuildHypergraphRequest { ... })`.

**Files touched.** New: `crates/rust-code-mcp-graph/**` (with split). Moved out
of server: `src/graph/*`. Edited: server tool handlers (~10 files in `tools/`).

**Verify.**
```
nix develop ../nix-devshells#code --command cargo check -p rust-code-mcp-graph
nix develop ../nix-devshells#code --command cargo test  -p rust-code-mcp-graph --lib
nix develop ../nix-devshells#code --command cargo tree  -p rust-code-mcp-graph -e normal --depth 1 \
  | grep -E '\b(rmcp|tantivy|lancedb|rust-code-mcp-search)\b'
# All four must return nothing. Graph must not pull rmcp, search, or search backends.
```

**Rollback.** Single commit revert; the file splits make `git revert` noisy
but git/jj handle it.

**Done when.** Server's `src/` has no `use heed::` and no `use crate::graph::`.
Graph crate's public API contains no `heed::*` types. `cargo tree` shows graph
crate independent of search crate (forbidden edge prevented).

---

### Phase 7 — Extract `rust-code-mcp-ide`

**Goal.** Thin facade over `-ra-host` exposing live IDE navigation. Tiny crate
(<500 LOC).

**Prereqs.** Phases 0, 1, 4.

**Steps.**

1. `mkdir -p crates/rust-code-mcp-ide/src`.
2. `Cargo.toml`:
   ```toml
   [package]
   name = "rust-code-mcp-ide"
   # ... workspace inheritance ...

   [dependencies]
   anyhow                = { workspace = true }
   rust-code-mcp-paths   = { workspace = true }
   rust-code-mcp-ra-host = { workspace = true }
   serde                 = { workspace = true }
   thiserror             = { workspace = true }
   tokio                 = { workspace = true, features = ["sync"] }
   tracing               = { workspace = true }

   [lints]
   workspace = true
   ```
3. Move from server: `src/semantic/{mod.rs, position.rs}` (the loader already
   moved to `-ra-host` in Phase 4). Add new file `src/lib.rs` exposing
   `IdeService`.
4. Public API:
   ```rust
   pub struct IdeService { /* clones a Handle into -ra-host */ }
   pub enum IdeError { ... }

   impl IdeService {
       pub fn new(host: Arc<HostLayer>) -> Self;
       pub async fn find_definition(&self, req: SymbolLookupRequest) -> Result<DefinitionOutput, IdeError>;
       pub async fn find_references(&self, req: SymbolLookupRequest) -> Result<ReferencesOutput, IdeError>;
       pub async fn get_dependencies(&self, req: FileAnalysisRequest) -> Result<DependenciesOutput, IdeError>;
       pub async fn get_call_graph(&self, req: CallGraphRequest) -> Result<CallGraphOutput, IdeError>;
       pub async fn analyze_complexity(&self, req: FileAnalysisRequest) -> Result<ComplexityOutput, IdeError>;
   }
   ```
   `IdeService` is `Clone` (cheap; the heavy state lives in the actor inside
   `HostLayer`). Concurrency: every method posts a closure into `-ra-host`
   actor and awaits.
5. **No `ra_ap_*` in public signatures.** `Analysis`, `AnalysisHost`,
   `FilePosition`, etc. stay inside `-ra-host`'s closure callbacks.
6. Per-workspace pool: a `Mutex<HashMap<PathBuf, WorkspaceHandle>>` inside
   `IdeService` caches handles. Crucially, the handle holding pattern is "hold
   for ~1 sync op" (the actor serializes salsa), not "hold across the entire
   query."
7. Server: remove `src/semantic/`. Add `rust-code-mcp-ide` dep. Build the
   single `IdeService` instance in `main`, store in `AppServices`.

**Files touched.** New: `crates/rust-code-mcp-ide/**`. Moved out of server:
`src/semantic/`. Edited: server `tools/analysis_tools.rs` and `main.rs`.

**Verify.**
```
nix develop ../nix-devshells#code --command cargo check -p rust-code-mcp-ide
nix develop ../nix-devshells#code --command cargo tree  -p rust-code-mcp-ide -e normal --depth 1 \
  | grep -E '\b(ra_ap_|heed|tantivy|lancedb)\b'
# All must return nothing — ide reaches RA only via -ra-host.
```

**Rollback.** Single commit revert.

**Done when.** Server's `src/` has no `src/semantic/` directory. `cargo tree -p rust-code-mcp-ide -e normal`
shows `rust-code-mcp-ra-host` and `rust-code-mcp-paths` as the only workspace
deps.

---

### Phase 8 — Server crate cleanup

**Goal.** With domain logic moved out, what remains in `-server` is rmcp wiring
+ Config + SyncManager + tool handlers. Split the 3,589-LOC `tools/graph_tools.rs`
+ the consolidated handler file (currently `tools/search_tool_router.rs`) into
v1's clean per-domain decomposition. Drop the `tool_bridge.rs` shim if any
phase introduced it (Phase 6 prevented this; this is a guard).

**Prereqs.** Phases 0–7.

**Steps.**

1. Split `crates/rust-code-mcp-server/src/tools/graph_tools.rs` (now in server
   after Phase 6 moved the *backing* logic to `-graph`; the handler shells stayed
   here) into:
   ```
   src/tools/graph/calls.rs     (who_calls, who_uses, calls_from, call_graph, ...)
   src/tools/graph/crates.rs    (crate_edges, crate_dependency_metric, ...)
   src/tools/graph/overlaps.rs  (overlaps, semantic_overlaps)
   src/tools/graph/reexports.rs (re_export_chain, get_reexports, pub_use_pub_type_audit)
   src/tools/graph/modules.rs   (module_tree, workspace_stats, dead_pub_*)
   src/tools/graph/audits.rs    (unsafe_audit, derive_audit, fn_body_audit, channel_capacity_audit, missing_docs_audit, recursion_check, mut_static_audit)
   src/tools/graph/build.rs     (build_hypergraph)
   src/tools/graph/similarity.rs (similar_to_item, semantic_overlaps — composes graph + search + embedding)
   src/tools/graph/mod.rs       (#[tool_router] impl, re-exports per-file routers if rmcp supports nesting; else: single impl block with `pub use self::calls::*;` etc.)
   ```
2. Same shape for `src/tools/search/`: `query.rs`, `index.rs`, `health.rs`,
   `clear_cache.rs`, `read_file.rs`, `similarity.rs`.
3. `src/tools/ide/`: `definition.rs`, `references.rs`, `dependencies.rs`,
   `call_graph.rs`, `complexity.rs`.
4. Single `ServerRouter` (or `SearchToolRouter` renamed for clarity) carries
   the `#[tool_router]` macro. If rmcp supports `#[tool_router]` across multiple
   `impl` blocks for one struct, use that — one block per file. If not, keep
   one big `impl` in `src/lib.rs` but have each handler be a 5–10 line shim
   calling `services.search.foo(...)` / `services.graph.bar(...)` / etc., with
   the *Params struct defined in the same file.
5. Consolidate Params types. All `*Params` (with `#[derive(Deserialize, schemars::JsonSchema)]`)
   live in server. No duplicate definitions in `-graph` or `-search`. Server
   maps Params → domain Request inline at the handler boundary.
6. Move `Config` to a single home: `src/config.rs`. Stop scattering across
   server + `-search`. The server constructs everything from Config in `main`
   and passes domain crates only their narrow config (e.g.
   `IndexerConfig { data_dir, max_file_size, ... }` to `-search`).
7. Move/keep `src/monitoring/health.rs` here (it imports both `-search` and
   `-graph` types — only the server can do that).
8. Move/keep `src/metrics/` here for now. If a future crate needs to emit
   metrics, extract to a `-metrics` leaf at that point — not before.
9. `SyncManager` (`src/mcp/sync.rs`) stays in server. Add graceful shutdown:
   `tokio_util::sync::CancellationToken` + `tokio::select!` on
   `service.waiting()` vs `signal::ctrl_c()` (v2 had this — port it).
10. Construct `AppServices` once in `main`:
    ```rust
    let embedder    = Arc::new(EmbeddingRuntime::new_production()?);
    let host        = Arc::new(HostLayer::new(HostConfig::default()));
    let search      = SearchService::new(config.search.clone(), embedder.clone())?;
    let graph       = GraphService::new(host.clone(), embedder.clone(), paths.clone());
    let ide         = IdeService::new(host.clone());
    let sync_mgr    = Arc::new(SyncManager::new(search.clone(), Duration::from_secs(300)));
    let services    = AppServices { search, graph, ide, sync_mgr };
    let router      = ServerRouter::new(services, cancel_token.clone());
    ```
11. Crate-level doc block per Rule 5.

**Files touched.** Inside `crates/rust-code-mcp-server/src/` only. New
subdirectories under `tools/`; the giant flat file split into ~24 small files.

**Verify.**
```
nix develop ../nix-devshells#code --command cargo check -p rust-code-mcp-server
# No single .rs file in tools/ should exceed ~300 LOC after the split.
find crates/rust-code-mcp-server/src/tools -name '*.rs' -exec wc -l {} \; | sort -rn | head -5
```

**Rollback.** Single commit revert; file splits are mechanical.

**Done when.** No file in `crates/rust-code-mcp-server/src/tools/` exceeds
~300 LOC. Param struct definitions appear exactly once each (grep for
`struct BuildHypergraphParams` returns one hit). `tool_bridge.rs` does not
exist anywhere.

---

### Phase 9 — Extract `rust-code-mcp-search-eval`

**Goal.** Restore v1's offline IR-quality evaluation crate. v2 wrongly inlined
this into `rcm-search/src/search/rrf_tuner.rs`; we put it back as an orphan
crate that the runtime cannot depend on.

**Prereqs.** Phases 0, 5.

**Steps.**

1. `mkdir -p crates/rust-code-mcp-search-eval/{src,tests}`.
2. `Cargo.toml`:
   ```toml
   [package]
   name = "rust-code-mcp-search-eval"
   # ... workspace inheritance ...

   [dependencies]
   anyhow                = { workspace = true }
   rust-code-mcp-search  = { workspace = true }   # for SearchResponse DTO adapter only
   serde                 = { workspace = true }
   serde_json            = { workspace = true }
   thiserror             = { workspace = true }

   [lints]
   workspace = true
   ```
3. Move from `-search` (if Phase 5 moved RRF tuner along with the rest): the
   tuner code, `EvaluationMetrics`, `NDCG/MRR/MAP/Recall/Precision` math, the
   `RankedSearch` adapter trait, `RrfCandidateRun`, `RrfTuningReport`. Lift
   these out of `-search` into this crate.
4. Public API: `EvaluationMetrics`, `TestQuery`, `TestDataset`, `RankedHit`,
   `RelevanceJudgment`, `QueryEvaluation`, `RrfTuner`, `RankedSearch`, plus
   the metric functions. Lift v1's surface verbatim.
5. Add `crates/rust-code-mcp-search-eval/tests/test_queries.json` (v1's
   12-query fixture) and `tests/evaluation.rs`.

**Files touched.** New: `crates/rust-code-mcp-search-eval/**`. Removed from
`-search`: `src/search/rrf_tuner.rs` and related (kept the runtime hybrid
search code; eval moves out).

**Verify.**
```
nix develop ../nix-devshells#code --command cargo check -p rust-code-mcp-search-eval
nix develop ../nix-devshells#code --command cargo test  -p rust-code-mcp-search-eval --tests
```

**Done when.** `-search-eval` is in `[workspace.members]`. No server file or
domain crate references it.

---

### Phase 10 — `xtask` + architecture-boundary tests + hygiene sweep

**Goal.** Move dev/CI scripting into a dedicated `xtask` crate (with the file
already split, unlike v2's 1,613-LOC mega-main). Add cargo-metadata-driven
boundary tests that make the hard rules CI-enforceable.

**Prereqs.** Phases 0–9.

**Steps.**

1. `mkdir -p xtask/src`.
2. `xtask/Cargo.toml`:
   ```toml
   [package]
   name = "xtask"
   # ... workspace inheritance, publish = false ...

   [dependencies]
   anyhow     = { workspace = true }
   cargo_metadata = { workspace = true }
   serde_json = { workspace = true }
   syn        = { workspace = true, features = ["full", "visit"] }
   toml       = { workspace = true }
   walkdir    = { workspace = true }

   [[bin]]
   name = "xtask"
   path = "src/main.rs"
   ```
3. Split from the start — `xtask/src/main.rs` is thin dispatch only:
   ```
   xtask/src/main.rs           (subcommand router, <100 LOC)
   xtask/src/baseline.rs       (warning-baseline subcommand)
   xtask/src/smoke.rs          (phase smoke tests: JSON-RPC harness)
   xtask/src/policy/mod.rs     (CLI for policy lints)
   xtask/src/policy/deps.rs    (no SDK types in capability APIs)
   xtask/src/policy/singletons.rs (LazyLock/OnceLock<Mutex<_>> scan)
   xtask/src/policy/forbidden_edges.rs (no search↔graph↔ide deps)
   ```
4. Add `crates/rust-code-mcp-server/tests/architecture_boundaries.rs`. Uses
   `cargo_metadata` to read every crate's dep list, then asserts:
   - `rust-code-mcp-search`, `-graph`, `-ide` do NOT depend on each other.
   - `rust-code-mcp-server` is the only crate with `rmcp` as a direct dep.
   - `rust-code-mcp-search-eval` is the only crate that has zero workspace
     consumers AND is allowed to depend on `-search`.
   - No crate depends on `rust-code-mcp-search-eval`.
   - `ra_ap_ide` family appears only in `-ra-host`'s direct deps.
   - `heed` appears only in `-graph`'s direct deps.
   - `tantivy`, `lancedb`, `sled` appear only in `-search`'s direct deps.
   - `fastembed`, `ort` appear only in `-embedding`'s direct deps.
   - `ra_ap_syntax` appears only in `-parse` and `-ra-host`'s direct deps.
5. `pub(crate)` sweep: run `cargo run -p xtask -- baseline` after a quick
   `dead_pub_report` audit (this repo has the MCP servers indexed already);
   downgrade everything not in a `lib.rs` `pub use` to `pub(crate)`. Estimated
   ~400 mechanical downgrades based on v2's surface.
6. Delete the `examples/` directory under the server crate. Convert the few
   genuinely useful ones (`load_benchmark.rs`, `ide_load_benchmark.rs`,
   `benchmark_phases.rs`) to integration tests under `crates/rust-code-mcp-search/tests/`
   or to `xtask` subcommands.
7. Delete `run_burn_test.sh` and `benchmark.sh` from the repo root; either
   move into `xtask` as subcommands or drop entirely.
8. Final README + TOOLS.md regeneration: hand-edit the README's
   "active workspace is organized around capability crates" table (v1 had it
   right); have TOOLS.md generated by `cargo run -p xtask -- docs --emit tools` to
   eliminate hand-edit drift across attempts.

**Files touched.** New: `xtask/**`,
`crates/rust-code-mcp-server/tests/architecture_boundaries.rs`. Deleted:
top-level `benchmark.sh`, `run_burn_test.sh`, monolith's
`crates/rust-code-mcp-server/examples/*` (most of them).

**Verify.**
```
nix develop ../nix-devshells#code --command cargo check -p xtask
nix develop ../nix-devshells#code --command cargo test  -p rust-code-mcp-server --test architecture_boundaries
nix develop ../nix-devshells#code --command cargo run -p xtask -- policy deps
nix develop ../nix-devshells#code --command cargo run -p xtask -- policy forbidden-edges
nix develop ../nix-devshells#code --command cargo run -p xtask -- policy singletons
```
All five must pass.

**Done when.** Architecture-boundary tests pass on CI. xtask is the only
non-source-tree binary entry point. No `examples/` directory at any level.


## 8. Success criteria (end of Phase 10)

A v3 layout is complete when ALL of these hold:

1. `cargo build --workspace --release` succeeds in under ~70% of the monolith's
   release build time (cold cache), measured by `cargo build --workspace --timings`.
2. Touching one file in `crates/rust-code-mcp-search/src/search/bm25.rs` and
   re-running `cargo check --workspace` does NOT trigger rebuilds of
   `-graph`, `-ide`, `-ra-host`, or `-embedding`. (cargo build graph respects
   the no-cross-capability rule.)
3. `cargo tree -p rust-code-mcp-search -e normal --depth 1` shows no `ra_ap_*`
   dep. `cargo tree -p rust-code-mcp-graph -e normal --depth 1` shows no
   `tantivy`, `lancedb`, `sled`, `rmcp` dep. Mirror checks for `-ide`.
4. Architecture boundary test passes:
   `cargo test -p rust-code-mcp-server --test architecture_boundaries`.
5. No `pub` item in any domain crate's `lib.rs` lacks an external consumer
   (where "external" means in another crate's `src/`). Verified via
   `dead_pub_report`.
6. Every `lib.rs` opens with a "What this crate owns / does not own" doc block.
7. No file in `crates/*/src/` exceeds ~700 LOC (the 3,571-LOC `queries.rs`
   is split; the 3,589-LOC `graph_tools.rs` is split; the 1,452-LOC
   `lib.rs` in v2's `rcm-server` is split).
8. `grep -rn 'LazyLock<Mutex<' crates/*/src/` returns no hits in production
   code. (Tests may keep them.)
9. `.mcp.toml` symlinks resolve to a working `rust-code-mcp` binary; the
   MCP server starts, exposes the same ~50 tools with identical names and
   schemas as the monolith, and a sanity smoke (`xtask phaseN-smoke`)
   succeeds against the fixture workspace.
10. `cargo run -p xtask -- baseline` reports zero warnings on a clean build.


## 9. Appendix A — Module → crate move cheatsheet

Mechanical reference for which monolith file goes where. Use this to drive each
phase; do not rely on memory.

| Monolith file (under `src/` today) | Goes to crate | New path under `crates/<crate>/src/` |
|---|---|---|
| `bin/test_tools_direct.rs` | delete in Phase 10 | (deleted) |
| `chunker/mod.rs` | search | `chunker.rs` |
| `config.rs` | server | `config.rs` |
| `config/{errors,indexer}.rs` | server | `config/{errors,indexer}.rs` (or fold into config.rs) |
| `embeddings/{mod,error}.rs` | embedding | `lib.rs` (single file) |
| `graph/{ast_resolve,attributes,bindings,extract,hir_trim,ids,impls,model,signatures,statics,storage,snapshot,usages}.rs` | graph | `{ast_resolve,attributes,bindings,extract,hir_trim,ids,impls,model,signatures,statics,storage,snapshot,usages}.rs` |
| `graph/queries.rs` (3,571 LOC) | graph | SPLIT to `queries/{calls,crates,overlaps,reexports,modules,attributes,metrics}.rs` |
| `graph/loader.rs` | ra-host | `loader.rs` (merged with `semantic/loader.rs`) |
| `graph/{channel,derive,docs,fn_body,recursion,unsafe}_audit.rs`, `graph/{mut_static,statics}_audit.rs` | graph | `audits/{channel,derive,docs,fn_body,recursion,unsafe,mut_static}.rs` |
| `indexing/{consistency,embedding_batcher,error,incremental,indexer_core,merkle,tantivy_adapter,unified,...}.rs` | search | `indexing/*` (verbatim) |
| `lib.rs` (monolith) | server | `lib.rs` (new contents per Phase 0) |
| `main.rs` | server | `main.rs` |
| `mcp/{mod,sync}.rs` | server | `mcp/{mod,sync}.rs` |
| `metadata_cache.rs` | search | `metadata_cache.rs` (pub(crate)) |
| `metrics/{mod,memory}.rs` | server (for now; extract leaf if 2nd consumer appears) | `metrics/` |
| `monitoring/{health,backup,mod}.rs` | server | `monitoring/` |
| `parser/{mod,call_graph,imports,type_references}.rs` | parse | `{mod,call_graph,imports,type_references}.rs` |
| `schema.rs` | search | `schema.rs` (pub(crate)) |
| `search/{bm25,hybrid,resilient,rrf,mod}.rs` | search (runtime); `rrf_tuner` and offline eval → search-eval | `search/*`; eval moves out in Phase 9 |
| `security/{mod,secrets}.rs` | search (pub(crate) submodule) | `security/` |
| `semantic/{mod,position,loader}.rs` | `mod.rs` and `position.rs` → ide; `loader.rs` → ra-host | `lib.rs` and `position.rs` in ide; merged into ra-host loader |
| `tools/*.rs` (handler dispatch) | server | `tools/{search,graph,ide,audits}/*.rs` (per Phase 8 split) |
| `vector_store/{lancedb,mod,traits,error}.rs` | search | `vector_store/` (pub(crate)) |


## 10. Appendix B — What we are explicitly NOT doing

- Keeping a `legacy` crate alive during migration. v2 tried this with
  `file-search-mcp-legacy` and Phases 2–7 dragged. We move per phase, not
  facade-then-fill.
- Adding a `core` / `common` / `domain-types` / `model` crate. The frozen plan
  and v1's plan-2 both name this as anti-goal. If two crates need a type,
  it lives in the natural owner; if three+ need it AND it is leaf-cheap, it
  goes in `-paths` (the only "shared utility" leaf we are allowed).
- Going async-everywhere. Domain methods are sync (chosen for testability +
  rust-analyzer's nature). Async appears at `*Service` method boundaries
  and at the rmcp transport edge; otherwise sync.
- Auto-generating MCP tool docs. Phase 10 mentions an `xtask docs --emit tools`
  step; that is a stretch goal, not a blocker. Hand-edited TOOLS.md is
  acceptable if the architecture-boundary test catches the structural drift
  that has bitten us so far.
- Renaming or removing MCP tools. The names in `TOOLS.md` are stable across
  the monolith, v1, and v2. We touch zero tool names.
- Replacing `anyhow` everywhere with `thiserror`. Domain crates use thiserror
  for their own typed errors; the server uses `anyhow` at the rmcp boundary
  via a `IntoMcpError` impl. This matches v1, which is the cleanest of the
  three on errors.


## 11. Appendix C — Phase summary table

| Phase | Title | Net new crate | Files moved | Risk | Est. effort |
|---:|---|---|---:|---|---:|
| 0 | Workspace skeleton | (server only) | ~all (rehome) | low | half day |
| 1 | -paths | yes | ~10 | low | half day |
| 2 | -parse | yes | 4 | low | half day |
| 3 | -embedding | yes | 2 | low | half day |
| 4 | -ra-host | yes | 2 + extensive imports | **high** | 2 days |
| 5 | -search | yes | ~50 | medium | 1.5 days |
| 6 | -graph | yes | ~22 + 2 file-splits | medium-high | 1.5 days |
| 7 | -ide | yes | 2 | low | half day |
| 8 | Server cleanup | – | – (in-place split) | low | 1 day |
| 9 | -search-eval | yes | ~5 | low | half day |
| 10 | xtask + boundary tests | yes | – (new) | low | 1 day |

Total budget: ~10 working days at sustained pace. Phase 4 is the swing phase;
do it on a day with no interruptions. Phases 1, 2, 3 can pipeline because they
have no inter-dependencies; if working alone, do them sequentially for clarity.


## 12. Open questions to resolve before starting

1. **Which Rust toolchain channel?** v1 + monolith are on nightly. v2 pinned
   stable 1.95.0. The plan above proposes stable 1.85.0 — confirm this is
   acceptable, or adjust. (Edition 2024 is stable as of 1.85.)
2. **rmcp version**. All three layouts pin
   `rmcp = { git = "modelcontextprotocol/rust-sdk", branch = "main" }`. Pin
   to a specific commit before starting, so a Phase-4 rebuild doesn't drag in
   an unrelated upstream change.
3. **ra_ap_* version**. All three layouts use 0.0.330. Keep through this
   refactor; a bump is a separate change.
4. **Per-workspace IDE pool size**. v2 uses 2–4. Keep at the same value as
   today's monolith setting, or default to `num_cpus / 2` capped at 4.
5. **Test suite gating**. Heavy snapshot tests cost ~115 s. Recommend:
   ordinary CI runs `cargo test --workspace --exclude rust-code-mcp-graph`;
   a separate `cargo test -p rust-code-mcp-graph --test queries_snapshot`
   job runs on a slower nightly schedule. Confirm CI willingness.
