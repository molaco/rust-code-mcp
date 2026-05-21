# Refactor Plan: Phase 7 — Cleanup & Crate Lift

Status: not ready to execute. Three sequential options (A → B → C), each optional after Phase A. Phase A is mandatory if any crate lift is later attempted. Do not start any phase until Phases 1-6 of `.plans/refactor-plan.md` (PRs 00-21) have settled — at minimum one normal-feature-work pass through the codebase with no further structural change. As of 2026-05-21 the refactor work landed but has not yet aged.

Basis: boundary analysis run on `/home/molaco/Documents/rust-code-mcp-refactor` after PR 21 (2026-05-21). The analysis surfaced data that the original `refactor-plan.md` §11 did not account for:

- **Two SCCs** in the module graph block any crate lift across their seams.
- `graph::codemap` depends on `embeddings` (via the PR 12-13 `SeedHit` DTO; the historical `crate::search::SearchResult` runtime coupling was removed at that point and only survives as documentation references). `rmc-graph` therefore cannot stand alone — it must sit above `rmc-engine`.
- `graph` has 66 declared re-exports (32 from `query::model::*` via glob); never reviewed as crate-API.

This plan operationalizes the analysis. It supersedes the §11 wording of the parent plan in the specific sense that the lift order is **engine first, then graph, then server**, not **graph alone**.

## 0. Goal

Get the codebase to a state where module boundaries are *crate-grade*: cycle-free, with deliberate (not accidental) public surfaces, and lifted into Cargo workspace crates **where the seam actually pays off**.

Three options, ordered:

- **Option A (Cleanup)** — fix the cycles, narrow `graph`'s surface, let the structure settle. After this the codebase is ready to lift but is not yet lifted. **This is the only mandatory option in this plan.**
- **Option B (Engine + Graph lift)** — lift `parser + chunker + embeddings + vector_store + search + schema` as `rmc-engine`, and `graph` as `rmc-graph`. Main crate keeps everything else.
- **Option C (Server lift)** — extend Option B by lifting the application/adapter layer too, in dependency order: `rmc-config` first, then `rmc-indexing` (indexing + monitoring + metadata_cache + metrics + security as one crate, because `indexing` directly consumes the utility three), then `rmc-server` (tools + mcp + semantic). Main crate becomes a binary + glue.

Each option is a checkpoint. Stop at A if there's no consumer pulling on a crate lift; reach for B only when there is; reach for C only after B has aged.

## 1. Evidence

### 1.1 Two SCCs

Verified 2026-05-21 with grep over the post-PR-21 `src/`:

```text
indexing ↔ monitoring     (2 edges, trivial)
  monitoring/backup.rs:5      use crate::indexing::merkle::FileSystemMerkle;
  indexing/unified.rs:308     backup_manager: Option<&crate::monitoring::backup::BackupManager>,

tools ↔ mcp                (6 edges, substantial)
  mcp/sync.rs:132             use crate::tools::project_paths::ProjectPaths;
  tools/router.rs:47,59,717   ...Arc<crate::mcp::SyncManager>...
  tools/endpoints/index.rs:156   ...Arc<crate::mcp::SyncManager>...
  tools/endpoints/query.rs:154,357   ...Arc<crate::mcp::SyncManager>...
```

Neither cycle violates §2's forbidden-edge list, which is why the refactor's structural cleanup didn't catch them — but both block a crate lift across the seam.

### 1.2 Public-surface inventory

Declared re-exports per top-level module (intentional API, from `get_declared_reexports`):

```text
graph           66    ← heaviest. 32 from query::model::* glob. Never API-reviewed.
embeddings      16    ← clean (backend, profile, openrouter config, token_lengths)
indexing         7    ← clean
parser           7    ← clean
chunker          5    ← clean
tools            4    ← minimal (SearchToolRouter, IndexCodebaseParams, index_codebase)
search           3    ← minimal (Bm25Search, ResilientHybridSearch, SearchError)
vector_store     3    ← minimal (LanceDbBackend, VectorStoreBackend, VectorStoreError)
```

Plus 90 remaining `dead_pub_in_crate` candidates — a mix of direct `graph` re-exports (everything reached via `pub use query::model::*;` is both re-exported AND in this list, because no in-repo consumer imports through the re-exported path) and types reachable only through public method signatures. 51 of the 90 are in `graph`. The list is the input to Phase A.3, not a hard target.

### 1.3 DAG layering (cycles collapsed)

```text
LEVEL 0 (sinks):   parser  schema  metadata_cache  metrics  security  semantic
LEVEL 1:           chunker  → parser
LEVEL 2:           embeddings  → chunker
LEVEL 3:           vector_store  → chunker, embeddings
                   config  → embeddings  (one-way; blocks config from foundation)
LEVEL 4:           search  → chunker, embeddings, schema, vector_store
LEVEL 5:           graph  → embeddings  (post-PR-12/13: SeedHit DTO removed the runtime search edge)
LEVEL 6 (SCC):     {indexing, monitoring}  → engine cluster
LEVEL 7 (SCC):     {tools, mcp}  → everything
```

`config → embeddings` (one site, `config/indexer.rs:40` imports `EmbeddingProfile`) is a one-way edge but it means `config` cannot be at a foundation layer below `embeddings`. It must live alongside or above the engine.

## 2. Guardrails

These hold for **every** phase (A, B, C). The §3 guardrails of the parent plan continue to apply; the additions below address crate-level concerns:

1. **No formatting.** No `cargo fmt` invocation, ever.
2. **No public-path renames** other than those an SCC-break demands. A symbol's external path is preserved by a facade `pub use` in main crate's `lib.rs` whenever a module is lifted into a crate.
3. **No visibility widening to make a move compile.** A lift that requires widening a `pub(crate)` to `pub` is a signal that the boundary is wrong — stop and reconsider.
4. **Each phase ends green.** `cargo check --all-targets` is green at every commit. All ~39 in-repo consumers (`examples/`, `tests/`, `src/bin/`) must continue to work without changes.
5. **One concern per commit.** A single SCC fix, a single module move, a single cluster of related re-export demotions.
6. **Don't proceed past a phase exit gate without one verification settle period.** "Settle" means a normal-feature-work pass through the codebase (≥1 unrelated PR landing) with no follow-up structural changes. Crate-level boundaries harden the API; settling exposes whether the surface is really right.
7. **`vendor/fastembed/` is never edited.**

### 2.1 Verification command

**Every shell command in this plan runs under the project Nix devshell.** Code blocks below that show bare `cargo`, `grep`, etc. are shorthand for the wrapped form:

```sh
nix develop ../nix-devshells#cuda-code --command <command>
```

The full verification gate is:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
```

After Phase B has begun, this checks the workspace including all member crates.

For per-cluster tests:

```sh
nix develop ../nix-devshells#cuda-code --command cargo test -p <crate> --lib
```

## 3. Phase A: Cleanup (Cycle Breaks + Surface Narrowing)

Purpose: bring the module graph to a true DAG and bring `graph`'s public surface to a deliberate-API state. Mandatory if any later lift is attempted; valuable as a standalone phase even if no lift ever follows.

Operation: `Lower` visibility + small `Move` for the cycle breaks. No file count changes beyond the SCC-break extractions.

### 3.A.1 — Break `indexing ↔ monitoring` cycle  ✅ DONE 2026-05-21

Status: complete. Created `src/indexing/backup.rs` (13 lines) with `pub(crate) trait Backup`; impl block added to `src/monitoring/backup.rs`; `UnifiedIndexer::index_directory_with_backup` parameter changed from `Option<&BackupManager>` to `Option<&dyn Backup>`. `grep -rn 'crate::monitoring' src/indexing/` returns zero. `cargo check --all-targets` green (pre-existing unrelated warnings only).

Edges (pre-A.1, for reference):

- `monitoring/backup.rs:5` reads `indexing::merkle::FileSystemMerkle` (read-side observer; one-way `monitoring → indexing` is fine and natural).
- `indexing/unified.rs:308` takes `Option<&monitoring::backup::BackupManager>` as a function parameter — this is the cycle edge.

Resolution: extract a small trait at the indexing-side boundary that `BackupManager` implements. `indexing::unified` takes `Option<&dyn Backup>` (or `Option<&impl Backup>`), removing the type dependency on `monitoring`.

The call site is exactly one method (`unified.rs:312` → `manager.create_backup(&merkle)`), so the trait is one method:

```rust
// src/indexing/backup.rs  (new flat module, sibling to indexing/unified.rs)
use crate::indexing::merkle::FileSystemMerkle;

pub trait Backup {
    fn create_backup(&self, merkle: &FileSystemMerkle) -> anyhow::Result<PathBuf>;
}
```

Then:

```rust
// src/indexing/unified.rs
backup_manager: Option<&dyn crate::indexing::backup::Backup>,
```

```rust
// src/monitoring/backup.rs
impl crate::indexing::backup::Backup for BackupManager {
    fn create_backup(&self, merkle: &FileSystemMerkle) -> anyhow::Result<PathBuf> {
        BackupManager::create_backup(self, merkle)
    }
}
```

The `monitoring → indexing` edge (for `FileSystemMerkle`) stays; the `indexing → monitoring` edge is gone. If the call shape grows (e.g. a future `restore_latest` path), add the method to the trait — keep the trait shape derived from real call sites, not speculative.

Commits:

- **A.1.a** — extract `trait Backup` into `src/indexing/backup.rs`; update `indexing::unified::UnifiedIndexer` signature; implement the trait on `BackupManager` in `src/monitoring/backup.rs`. Update the call sites.

Verification:

```sh
grep -rn 'crate::monitoring' src/indexing/  # must be empty
cargo check --all-targets
```

Risk: Low. The trait is small (1–3 methods) and the dependency direction inverts cleanly.

### 3.A.2 — Break `tools ↔ mcp` cycle  ✅ DONE 2026-05-21

Status: complete. Moved `src/tools/project_paths.rs` → `src/mcp/project_paths.rs` (303 lines, content preserved). Visibility on the four helpers widened to `pub(crate)` per the narrowest-expressible rule above. Compat shim left at `src/tools/project_paths.rs` (3 lines: `pub use crate::mcp::project_paths::*;`). 9 in-crate caller imports rewritten across `mcp/sync.rs`, `tools/endpoints/{health,cache,index,query,indexing_support}.rs`, `tools/graph/{codemap,similarity}.rs`. `grep -rn 'use crate::tools' src/mcp/` returns zero (cycle gone). `cargo check --all-targets` green.


Edges:

- `mcp/sync.rs:132` reads `tools::project_paths::ProjectPaths` — this is the cycle edge.
- `tools/router.rs:{47,59,717}` and `tools/endpoints/{query,index}.rs` hold `Arc<mcp::SyncManager>` — natural one-way `tools → mcp` (adapter layer depends on session manager).

Resolution: relocate `ProjectPaths` (and its private helper functions) from `tools::project_paths` to `mcp::project_paths`, **leaving a compatibility shim at the original path so in-repo consumers (notably `tests/test_mcp_stdio_transport.rs:9` which imports `rust_code_mcp::tools::project_paths::ProjectPaths`) keep working**. `mcp` is *lower* in the adapter stack (fan-out 2, used by tools); putting project-discovery types there makes `mcp` self-contained and `tools → mcp` the only direction.

Concrete shape:

- **Move target**: `src/tools/project_paths.rs` → `src/mcp/project_paths.rs`.
- **Visibility migration**: `src/tools/project_paths.rs` currently declares four helpers as `pub(in crate::tools)` (at lines 36, 42, 190, 201: `data_dir`, `resolve_embedding_backend`, `dir_hash`, `read_embedder_identity`). After the move these become invalid (the module is no longer inside `crate::tools`).

  **Caller inventory** (verified by grep on 2026-05-21):
  - `data_dir` — 9 sites across `tools/endpoints/{health,cache,indexing_support}.rs`.
  - `resolve_embedding_backend` — 5 sites across `tools/endpoints/{index,query}.rs` and `tools/graph/similarity.rs`.
  - `dir_hash` — 3 sites in `tools/endpoints/cache.rs`.
  - `read_embedder_identity` — 4 sites in `tools/endpoints/{health,query}.rs`.

  All callers live in `crate::tools::*`, a sibling subtree to `crate::mcp::*` (not a descendant or ancestor). Neither `pub(in crate::mcp)` (mcp-subtree only) nor `pub(super)` (parent only) suffices. The narrowest **expressible** Rust visibility preserving these callers is `pub(crate)`.

  Apply `pub(crate)` to all four helpers as the narrowest-expressible-visibility resolution of the move. Justification under Guardrail 3 of the parent plan:
  - `pub(in crate::tools)` restricted reachability to one specific subtree; Rust has no syntax for "visible from `crate::tools::*` when the item lives in `crate::mcp::*`."
  - The actual reachability set (caller files + call sites) does not change; only the textual visibility marker widens.
  - Refactoring the four helpers into associated `fn`s on `ProjectPaths` would make them `pub` (since `ProjectPaths` is `pub`) — strictly wider than `pub(crate)`. `pub(crate)` is the narrower of the two viable options.

  Record the widening in the A.2 commit message. If a future phase reduces the caller set so a narrower form becomes expressible, revisit.
- **Compat shim**: `src/tools/project_paths.rs` becomes a one-line facade:
  ```rust
  pub use crate::mcp::project_paths::*;
  ```
  This preserves `rust_code_mcp::tools::project_paths::ProjectPaths` for `tests/test_mcp_stdio_transport.rs` and any other in-repo consumer. The shim is deleted in Phase C.3 (when `tools` lifts to `rmc-server` and the test must be either migrated or kept resolving via the main `lib.rs` facade).
- **Declare in `src/mcp/mod.rs`**: `pub mod project_paths;`.
- **In-crate caller rewrites**: `use crate::tools::project_paths::ProjectPaths;` → `use crate::mcp::project_paths::ProjectPaths;` for every caller inside `src/`. The shim handles external (`tests/`, `examples/`) consumers; in-crate callers should resolve through the canonical path so the cycle is genuinely broken.

Commits:

- **A.2.a** — relocate `project_paths` module; rewrite visibility markers; add compat shim; rewrite in-crate caller imports.

Verification:

```sh
grep -rn 'use crate::tools' src/mcp/         # must be empty (the cycle edge)
grep -rn 'use crate::tools::project_paths' src/   # must be empty (in-crate callers migrated)
grep -n  'pub use crate::mcp::project_paths' src/tools/project_paths.rs   # must show the shim
cargo check --all-targets    # tests/test_mcp_stdio_transport.rs continues to compile via the shim
```

Risk: Medium. The visibility audit on the four `pub(in crate::tools)` helpers is the subtle part; widening any of them to `pub`/`pub(crate)` violates Guardrail 3. Stage in two sub-commits if the diff is hard to review:

- A.2.a.1: move file + add shim + declare in `mcp/mod.rs` (cycle still present via in-crate callers).
- A.2.a.2: rewrite in-crate caller imports; visibility audit.

### 3.A.3 — Narrow `graph` public surface  ✅ DONE 2026-05-21

Status: complete. Removed the `pub use query::model::*;` glob from `src/graph/mod.rs`. Surface narrowed from 66 declared re-exports → 42 individual items (38 `pub` cross-crate API, 3 `pub(crate)` graph-internal, 22 dead items removed from the facade). Additionally narrowed three module declarations: `mod query;`, `mod math;`, `mod embedding_cache;` are now private at the module level — only reachable via the explicit re-exports. Cross-crate-API decisions pinned by callers in `src/tools/`, `tests/`, `examples/`. `cargo check --all-targets` green.


Goal: reduce `graph`'s 66 declared re-exports + 51 transitively-reachable pubs to a deliberate ~30-export contract. The 32 `query::model::*` types that arrive via glob (`pub use query::model::*;`) are the worst offenders — most are tool-response shapes (`DeadPubFinding`, `CrateEdge`, `EnrichedCallSite`, …) consumed by `tools/graph/*` endpoints by name.

Sub-process (one cluster per commit):

- **A.3.a** — Audit `query::model` exports. For each of the 32 types, run `who_imports rust_code_mcp::graph::query::model::<T>` and `who_uses rust_code_mcp::graph::<T>`. **Classify against the future crate boundary (Phase B end-state), not the current single-crate state:**
  - **Cross-crate API**: type is used by any module that will land outside `rmc-graph` after Phase B — i.e. anything in `tools::`, `mcp::`, `indexing::`, the binary, or in-repo examples/tests. Keep `pub`. Example: `tools/graph/crates.rs:11` imports `graph::{CrateEdge, CrateMetric, ForbiddenDependencyViolation}`; all three must stay `pub` because `tools` becomes `rmc-server` while `graph` becomes `rmc-graph`.
  - **Graph-internal**: type used only by other modules that will remain inside `rmc-graph` (i.e. other files under `src/graph/`). Narrow to `pub(crate)` — still accessible inside the future crate.
  - **Dead-by-shadowing**: superseded by another type or never referenced. Delete.

  Critical: even if `who_uses` shows "one tool uses this," that tool will be in a different crate; the type still needs `pub`. The demotion rule is about *crate boundaries*, not consumer count.

- **A.3.b** — Replace `pub use query::model::*;` glob in `graph/mod.rs` with explicit named re-exports of the "Real API" set. The glob was a convenience during the refactor; an explicit list is the contract.

- **A.3.c** — Repeat the audit for the smaller-but-still-large clusters:
  - `graph::model::*` (13 dead-pub findings): `Binding`, `BindingKind`, `EmbeddingRecord`, `ExtractionModel`, `FunctionSignature`, `GenericBound`, `ItemKind`, `Namespace`, `Param`, `SelfKind`, `StaticMetadata`, `Usage`, `UsageCategory`.
  - `graph::storage::{GraphDatabases, GraphManifest, GraphEnvOptions}`.
  - `graph::ids::{NodeId, BindingId, UsageId}`.

- **A.3.d** — Same for `graph::*_audit` finding structs (`DeriveAuditOpts`, `DocsAuditOpts`, `UnsafeFinding`).

Target end-state: **`graph/mod.rs` has no glob re-exports**; every entry in `get_declared_reexports rust_code_mcp::graph` is a deliberate, individually-reviewed API decision. The post-review count is whatever the audit confirms — a large confirmed contract is a successful outcome; arbitrary demotion that breaks real consumers is not. The exit gate is "every export is on the list because someone defended it," not a target count.

Verification per commit:

```sh
cargo check --all-targets
mcp__rust-code-mcp__get_declared_reexports module=rust_code_mcp::graph
mcp__rust-code-mcp__dead_pub_in_crate krate=rust_code_mcp
```

Risk: Medium-high. Tests and examples consume many of these types by qualified name. Each demotion needs a `who_imports` check first.

### 3.A.4 — Settle pass

Do not start Phase B until the codebase has aged through one normal-feature-work pass after A.3 lands. This is the "one full verification pass unchanged" condition the parent plan §12 mandates. There is no command for this; it is a calendar wait. Use the time to verify:

- No new SCCs introduced by unrelated feature work.
- No new `pub` items added that shouldn't be `pub(crate)`.
- The narrowed `graph` surface hasn't been re-widened by accident.

Exit condition: cycles broken, `pub_crate_share` for `graph` meaningfully higher than its share of total `pub` was before A.3, codebase has aged.

## 4. Phase B: Engine + Graph Crate Lift

Purpose: lift the engine cluster (parser, chunker, embeddings, vector_store, search, schema) into one crate `rmc-engine`, and lift `graph` into its own crate `rmc-graph` that depends on it. Main crate keeps everything else.

Operation: `Lift`. Each move is a workspace-member relocation, not a code change.

Precondition: Phase A complete and settled.

### 4.B.0 — Workspace skeleton + dependency extraction strategy

Convert the single-crate repo into a Cargo workspace:

```toml
# Cargo.toml (root)
[workspace]
members = [".", "crates/rmc-engine", "crates/rmc-graph"]
resolver = "2"

[workspace.dependencies]
# extracted from the current top-level [dependencies] of the main crate.
# Each member crate selects the subset it actually uses via `workspace = true`.
anyhow = "..."
serde = { version = "...", features = ["derive"] }
tokio = { version = "...", features = [...] }
tantivy = "..."
heed = "..."
lancedb = "..."
# ...etc., all current top-level deps moved here

# [package] block for the main crate stays here

# [dependencies] for the main crate now includes:
rmc-engine = { path = "crates/rmc-engine" }
rmc-graph  = { path = "crates/rmc-graph" }
# plus whatever third-party deps the main crate's remaining code still uses directly.
```

**Per-crate dependency extraction is mandatory.** Each new member crate's `Cargo.toml` declares a focused `[dependencies]` block containing **only the third-party crates that crate's `src/` actually consumes**. Determine this by:

1. `grep -rohE 'use [a-z_][a-z0-9_]*' crates/<crate>/src/ | sort -u` — list every extern-crate root referenced in the moved sources.
2. Filter to those that resolve to entries in the workspace `[workspace.dependencies]` (vs `std::`, `core::`, in-workspace crates, or std re-exports).
3. Declare each in `crates/<crate>/Cargo.toml` as `<dep> = { workspace = true }` plus the feature flags that crate's code uses (do not just inherit all features — narrow them).

Known dependency clusters surfaced by the boundary analysis (illustrative — verify before each lift):

- **`rmc-engine`**: `parser/` needs `syn` + `ra_ap_*`; `chunker/` needs `tree-sitter`/tokenizer crates; `embeddings/` needs `reqwest`/`hf-hub`/`tokenizers`/`candle-*`; `vector_store/` needs `lancedb` + `arrow`; `search/` needs `tantivy`; `schema.rs` needs `tantivy`.
- **`rmc-graph`**: `heed` (LMDB), `serde`, `anyhow`, plus `rmc-engine` workspace-path dep.
- **`rmc-config`** (Phase C): minimal — `serde`, `anyhow`, plus `rmc-engine` for `EmbeddingProfile`.
- **`rmc-indexing`** (Phase C): `tantivy`, `sled` (for `metadata_cache`), `walkdir`, `rayon`, plus `rmc-engine` + `rmc-config`.
- **`rmc-server`** (Phase C): `rmcp`, `tokio`, `tracing`, `ra_ap_ide`/`ra_ap_vfs` (for `semantic`), plus all four sibling workspace crates.

Treat the dependency extraction as a first-class step of each B/C sub-phase — not "relocation, not a code change." Cargo will not compile a moved module if its third-party deps aren't in the new crate's manifest.

Commits:

- **B.0.a** — Create `crates/rmc-engine/Cargo.toml` (initially with `[dependencies]` empty) + empty `src/lib.rs`; add workspace section to root `Cargo.toml`; move current top-level `[dependencies]` to `[workspace.dependencies]` in root. Verify `cargo check --workspace` is green (main crate continues to compile against the workspace-deps redirection).
- **B.0.b** — Create `crates/rmc-graph/Cargo.toml` + empty `src/lib.rs`. Same verification.

Exit: workspace builds; both new crates are empty stubs; `[workspace.dependencies]` carries every third-party dep so member crates can opt in.

### 4.B.1 — Lift sinks (parser, schema)

These are leaves with no top-level outgoing dependencies. Move both in one commit because they have no inter-dependency and the diff is mechanical.

Layout target:

```text
crates/rmc-engine/
  Cargo.toml
  src/
    lib.rs            # pub mod parser; pub mod schema;
    parser/
      mod.rs
      types.rs
      rust_parser.rs
      call_graph.rs
      imports.rs
      type_references.rs
    schema.rs
```

Compatibility re-export in main crate's `src/lib.rs`:

```rust
pub use rmc_engine::parser;
pub use rmc_engine::schema;
```

This keeps every existing `use crate::parser::…` / `use rust_code_mcp::parser::…` in main, examples, tests working without edits.

Commits:

- **B.1.a** — Move `src/parser/` → `crates/rmc-engine/src/parser/`; declare `pub mod parser;` in `rmc-engine/src/lib.rs`; **add `parser`'s third-party deps to `crates/rmc-engine/Cargo.toml`** (`syn`, `ra_ap_*` per the §4.B.0 extraction procedure); add `pub use rmc_engine::parser;` to main `src/lib.rs`.
- **B.1.b** — Move `src/schema.rs` → `crates/rmc-engine/src/schema.rs`; declare `pub mod schema;`; **add `tantivy` to `rmc-engine/Cargo.toml`** with the same feature set the main crate used; main re-export.

**Every subsequent B.x / C.x move follows the same three-part pattern: file move + module declaration + dependency extraction in the new crate's Cargo.toml.** Skipping the manifest step is the most likely cause of `cargo check` failures during the lift; treat it as a required sub-step.

Risk: Low. Both modules have no in-crate dependencies that change with the lift.

Verification: `cargo check --all-targets`.

### 4.B.2 — Lift `chunker`

`chunker` depends on `parser`. After B.1, `parser` is in `rmc-engine`, so `chunker`'s `use crate::parser::…` becomes `use crate::parser::…` *inside `rmc-engine`* — same crate scope, same imports. No code change inside the module.

Layout: add `chunker/` under `crates/rmc-engine/src/`. Declare `pub mod chunker;` in `lib.rs`. Add `pub use rmc_engine::chunker;` to main `src/lib.rs`.

Commit:

- **B.2.a** — Move `src/chunker/` → `crates/rmc-engine/src/chunker/`.

### 4.B.3 — Lift `embeddings`

`embeddings` depends on `chunker` (post-B.2 also in `rmc-engine`). One important wrinkle: `config::indexer` imports `embeddings::EmbeddingProfile`. After this lift, `config` is in main crate and must depend on `rmc_engine`. Main crate already depends on rmc-engine (added in B.0), so `config/indexer.rs`'s `use crate::embeddings::EmbeddingProfile;` continues to resolve via the re-export — but only if the `pub use rmc_engine::embeddings;` is added to `src/lib.rs` in this commit.

Commit:

- **B.3.a** — Move `src/embeddings/` → `crates/rmc-engine/src/embeddings/`; add `pub mod embeddings;` to engine `lib.rs`; add `pub use rmc_engine::embeddings;` to main `lib.rs`.

### 4.B.4 — Lift `vector_store`

`vector_store` depends on `chunker` + `embeddings` (both in `rmc-engine` by now).

Commit:

- **B.4.a** — Move `src/vector_store/` → `crates/rmc-engine/src/vector_store/`; declare; re-export.

### 4.B.5 — Lift `search`

`search` depends on `chunker, embeddings, schema, vector_store` — all in `rmc-engine`. After this move, `rmc-engine` is internally complete.

Commit:

- **B.5.a** — Move `src/search/` → `crates/rmc-engine/src/search/`; declare; re-export.

### 4.B.6 — Verify engine boundary

Before lifting graph, verify `rmc-engine` is self-contained:

- `cargo check -p rmc-engine` green standalone.
- No `use crate::graph::`, `use crate::tools::`, `use crate::indexing::`, `use crate::mcp::`, `use crate::config::` anywhere in `crates/rmc-engine/src/`.

If any of those greps return a hit, that's a hidden inversion — stop and audit. The Phase A.1/A.2 cycle breaks were the *known* inversions; an unknown one would surface here.

### 4.B.7 — Lift `graph`

`graph` depends on `embeddings` (the runtime `crate::search::*` coupling was removed in PR 12-13 via the `SeedHit` DTO; remaining `crate::search` mentions in `graph/` are doc comments and test fixture string literals — `grep -rn 'use crate::search' src/graph/` returns zero hits). `rmc-graph/Cargo.toml` adds only `rmc-engine = { path = "../rmc-engine" }`.

Inside `graph`, every `use crate::embeddings::…` becomes `use rmc_engine::embeddings::…`. This is the only code change in this commit. Doc comments mentioning `crate::search::SearchResult` are left in place (they explain `SeedHit`'s historical motivation) or optionally updated to `rmc_engine::search::SearchResult` for accuracy.

Commit:

- **B.7.a** — Move `src/graph/` → `crates/rmc-graph/src/graph/`; declare `pub mod graph;` in `rmc-graph/src/lib.rs`; rewrite cross-crate imports inside `graph/` from `crate::embeddings::…` to `rmc_engine::embeddings::…`; add `pub use rmc_graph::graph;` to main `src/lib.rs`.

Risk: Medium. The cross-crate import rewrite is concentrated (embeddings is the only foreign edge). Stage in two sub-commits if needed:

- B.7.a.1: move files, declare module, accept compile errors.
- B.7.a.2: rewrite all `use crate::embeddings::` → `use rmc_engine::embeddings::` inside `crates/rmc-graph/src/`.

Pre-commit gate before B.7.a closes: `grep -rn 'use crate::\(search\|indexing\|tools\|mcp\|config\)' crates/rmc-graph/src/` must return zero. Any hit means the lift exposed a hidden inversion that Phase A did not catch — stop and audit.

### 4.B.8 — Verify with `forbidden_dependency_check`

After B.7, `forbidden_dependency_check` becomes meaningful for the first time — it operates on crate edges, which now exist.

```text
forbidden_dependency_check rules:
  rmc-engine    → may NOT depend on rmc-graph
  rmc-engine    → may NOT depend on rust-code-mcp (main)
  rmc-graph     → may NOT depend on rust-code-mcp (main)
  rmc-graph     → MAY depend on rmc-engine (sanctioned)
  rust-code-mcp → MAY depend on rmc-engine, rmc-graph
```

Plus the original §2 forbidden edges (now machine-enforceable):

```text
graph crate    → may NOT depend on tools/mcp (encoded by the fact that those are in main crate)
engine crate   → may NOT depend on indexing/tools/mcp (same)
embeddings    → may NOT depend on indexing  (within rmc-engine: enforce by grep)
```

Commit:

- **B.8.a** — Add a CI check or workspace-level test that runs `forbidden_dependency_check` and asserts zero violations. (`rust-code-mcp` itself is the tool that provides this check, so this is dogfooding.)

Risk: Low. By the time we're here, the boundaries already work.

### 4.B Exit conditions

- `cargo check --workspace --all-targets` green.
- `cargo test --workspace --all-targets` green (no regressions in examples or tests).
- `forbidden_dependency_check` returns zero violations against the §4.B.8 rule set.
- `rmc-engine` and `rmc-graph` each have a written `README.md` describing the crate's purpose, dependency direction, and stability story.
- Codebase has aged through one settle pass (≥1 unrelated PR landing after B.8).

If any of these fail, Phase C is not yet eligible.

## 5. Phase C: Server Cluster Lift

Purpose: lift the application/adapter layer (`indexing + monitoring`, `tools + mcp`, plus the small adapter-utility modules) so that the main crate becomes just the binary entry point + glue. After Phase C, the main crate's `src/` directory is much smaller than today.

Operation: `Lift`. Each move is a workspace-member relocation; SCCs are pre-broken in Phase A, so no code-restructuring work remains.

Precondition: Phase B complete and settled.

### 5.C.0 — Re-baseline

Re-run the boundary analysis on the post-B codebase. Look for:

- New cycles introduced by feature work during the settle pass.
- New `crate::tools` / `crate::mcp` references in `rmc-engine` or `rmc-graph` (should be zero — `forbidden_dependency_check` enforces this, but verify).
- The state of the `indexing ↔ monitoring` and `tools ↔ mcp` boundaries — did anyone accidentally widen them again? Re-verify with grep.

Exit: confirmed no new cycles; ready to lift.

### Lift ordering for Phase C

`indexing` directly imports `chunker, config, embeddings, metadata_cache, metrics, parser, schema, security, vector_store` (verified by `grep -rohE 'use crate::[a-z_]+' src/indexing/`). After Phase B, only the engine subset (`chunker, embeddings, parser, schema, vector_store`) lives in a workspace crate; `config, metadata_cache, metrics, security` still live in the main crate. If `indexing` is lifted before those, `rmc-indexing` would have to depend on the main crate — a cycle (main already depends on `rmc-indexing` for the binary).

Phase C therefore lifts in this order:

1. **C.1** — `rmc-config` (single-module crate; depends only on `rmc-engine`).
2. **C.2** — `rmc-indexing` carrying `indexing`, `monitoring`, **and** the adapter-utility modules it consumes (`metadata_cache`, `metrics`, `security`) — all in one crate / one commit because they form indexing's natural dependency set.
3. **C.3** — `rmc-server` carrying `tools`, `mcp`, `semantic` (and anything left in main other than the binary).
4. **C.4** — Main crate collapses to binary + glue.

### 5.C.1 — Lift `config` as `rmc-config`

`config` consumes `embeddings::EmbeddingProfile` (verified: `src/config/indexer.rs:40`). After Phase B that lives in `rmc-engine`, so `rmc-config` depends on `rmc-engine` only.

Reason for a separate crate (not folded into `rmc-indexing`): `config` and `indexing` are different concerns — separation makes ownership cleaner and lets a future consumer use config without pulling indexing in. (`tools → indexing` is a real edge regardless: `tools/endpoints/index.rs` invokes the indexer, so `rmc-server` depends on `rmc-indexing` either way. The choice of where `config` lives does not change that.)

Layout:

```text
crates/rmc-config/
  Cargo.toml         # [dependencies] rmc-engine = { path = "../rmc-engine" }
  src/
    lib.rs           # pub mod config;
    config/
      mod.rs
      indexer.rs
      errors.rs
```

Inside `config/indexer.rs`, rewrite `use crate::embeddings::EmbeddingProfile;` → `use rmc_engine::embeddings::EmbeddingProfile;`.

Compatibility re-export in main `src/lib.rs`: `pub use rmc_config::config;`.

Commit:

- **C.1.a** — Create `crates/rmc-config`; move `src/config/`; rewrite the engine import; add the main re-export.

Risk: Low. Three files; one foreign import to rewrite.

### 5.C.2 — Lift `indexing + monitoring + metadata_cache + metrics + security` as `rmc-indexing`

All five modules go into `rmc-indexing` together because `indexing` directly consumes the utility three (`metadata_cache`, `metrics`, `security`) and reaches `monitoring` post-A.1 via the `Backup` trait that lives in `indexing` itself. Lifting them piecewise would either create transient main-crate ↔ rmc-indexing cycles, or force a separate crate for each utility leaf — unjustified given their tiny size.

Layout target:

```text
crates/rmc-indexing/
  Cargo.toml         # depends on rmc-engine, rmc-config
  src/
    lib.rs           # pub mod indexing; pub mod monitoring; pub mod metadata_cache; pub mod metrics; pub mod security;
    indexing/        # moved from src/indexing/
    monitoring/      # moved from src/monitoring/
    metadata_cache/  # moved from src/metadata_cache/
    metrics/         # moved from src/metrics/
    security/        # moved from src/security/
```

The dependency direction inside `rmc-indexing` post-A.1, post-move:

```text
monitoring → indexing::{backup::Backup, merkle::FileSystemMerkle}   (one-way; this is the surviving direction post-A.1)
indexing → metadata_cache, metrics, security                         (one-way; utilities)
```

`indexing` no longer depends on `monitoring` at all: A.1 inverted the `indexing::unified::index_directory_with_backup` parameter from `Option<&monitoring::backup::BackupManager>` to `Option<&dyn crate::indexing::backup::Backup>`, so the type-level edge from indexing to monitoring is gone. Only `monitoring → indexing` remains (it needs the trait and `FileSystemMerkle` to implement the trait). The §3.A.1 cycle break is what makes this clean lift possible.

Import rewrites in `indexing/*` and `monitoring/*` (sweeping but mechanical):

- `use crate::chunker::…`         → `use rmc_engine::chunker::…`
- `use crate::embeddings::…`      → `use rmc_engine::embeddings::…`
- `use crate::parser::…`          → `use rmc_engine::parser::…`
- `use crate::schema::…`          → `use rmc_engine::schema::…`
- `use crate::vector_store::…`    → `use rmc_engine::vector_store::…`
- `use crate::config::…`          → `use rmc_config::config::…`
- `use crate::metadata_cache::…`  → `use crate::metadata_cache::…`  (same crate now — no change)
- `use crate::metrics::…`         → `use crate::metrics::…`         (same crate)
- `use crate::security::…`        → `use crate::security::…`        (same crate)

Compatibility re-exports in main `src/lib.rs`:

```rust
pub use rmc_indexing::{indexing, monitoring, metadata_cache, metrics, security};
```

Commit:

- **C.2.a** — Create `crates/rmc-indexing`; move all five modules together; rewrite cross-crate imports; add main re-exports. The `Backup` trait extraction from A.1 must already be in place — without it the `indexing ↔ monitoring` cycle re-emerges as a `indexing ↔ rmc-indexing::monitoring` cycle (still inside one crate, so it compiles, but the boundary intent is violated).

Stage in sub-commits if the diff is too large to review:

- C.2.a.1: move files, accept compile errors.
- C.2.a.2: rewrite imports in `indexing/`.
- C.2.a.3: rewrite imports in `monitoring/` (the few left after A.1).
- C.2.a.4: add main-crate re-exports.

Risk: Medium-high. `indexing::unified` has the largest fan-out in the codebase (11 targets); the import-rewrite touches more lines than any other single commit in this plan.

### 5.C.3 — Lift `tools + mcp + semantic` as `rmc-server`

Three modules go into `rmc-server`:

- `tools` — the MCP adapter endpoints.
- `mcp` — `SyncManager` + (post-A.2) `project_paths`.
- `semantic` — 4 files, used only by `tools`.

Layout target:

```text
crates/rmc-server/
  Cargo.toml         # depends on rmc-engine, rmc-graph, rmc-config, rmc-indexing
  src/
    lib.rs           # pub mod tools; pub mod mcp; pub mod semantic;
    tools/
    mcp/
    semantic/
```

Import rewrites in `tools/`:

- `use crate::{chunker,embeddings,parser,schema,search,vector_store}::…`     → `use rmc_engine::…`
- `use crate::graph::…`           → `use rmc_graph::graph::…`
- `use crate::config::…`          → `use rmc_config::config::…`
- `use crate::{indexing,monitoring,metadata_cache,metrics,security}::…` → `use rmc_indexing::…`
- `use crate::{mcp,semantic}::…`  → `use crate::{mcp,semantic}::…`  (same crate now)

Compatibility re-exports in main `src/lib.rs`:

```rust
pub use rmc_server::{tools, mcp, semantic};
```

Commit:

- **C.3.a** — Create `crates/rmc-server`; move the three modules; rewrite cross-crate imports; add main re-exports.

Risk: High. `tools` is the most-connected module in the codebase; the cross-crate import-rewrite is the largest diff of any commit in this plan.

Stage in sub-commits:

- C.3.a.1: move files, accept compile errors.
- C.3.a.2: rewrite imports in `tools/`.
- C.3.a.3: rewrite imports in `mcp/`.
- C.3.a.4: rewrite imports in `semantic/`.
- C.3.a.5: add main-crate compatibility re-exports.

### 5.C.4 — Main crate reduces to binary + glue

After C.3, the main crate's `src/` directory contains:

```text
src/
  lib.rs           # facade re-exports of all member crates' public surfaces
  main.rs          # binary entry point
  bin/
    test_tools_direct.rs
```

`src/lib.rs` becomes a thin facade:

```rust
pub use rmc_engine::{chunker, embeddings, parser, schema, search, vector_store};
pub use rmc_graph::graph;
pub use rmc_config::config;
pub use rmc_indexing::{indexing, monitoring, metadata_cache, metrics, security};
pub use rmc_server::{tools, mcp, semantic};
```

This keeps every existing `rust_code_mcp::tools::…` path resolving for the 26 examples + 12 tests + 1 binary.

Commit:

- **C.4.a** — Simplify `src/lib.rs` to facade-only; verify all examples/tests still build.

### 5.C Exit conditions

- `cargo check --workspace --all-targets` green.
- `cargo test --workspace --all-targets` green.
- `forbidden_dependency_check` returns zero violations against the full §2 rule set, now expressible at crate granularity:
  - `rmc-engine` depends on nothing in the workspace.
  - `rmc-graph` depends only on `rmc-engine`.
  - `rmc-config` depends only on `rmc-engine`.
  - `rmc-indexing` depends only on `rmc-engine`, `rmc-config`.
  - `rmc-server` depends on `rmc-engine`, `rmc-graph`, `rmc-config`, `rmc-indexing`.
  - **Main binary crate's `Cargo.toml` lists `rmc-engine`, `rmc-graph`, `rmc-config`, `rmc-indexing`, `rmc-server` as direct path dependencies** — Cargo requires direct deps for any crate named in `src/lib.rs`, and the facade re-exports five workspace crates. Transitive-only deps would compile but the `pub use` re-exports in §5.C.4 would not resolve.
- Main crate's `src/` has fewer than 5 files (lib.rs, main.rs, bin/*).
- Each new crate has a `README.md`.

## 6. Phase Output Template

Each phase reports:

```text
Option (A / B / C):
Sub-phase:
Operation (Lower / Move / Lift):
Files touched:
Cycle / boundary reason (what is being broken / lifted):
Compatibility paths preserved (facades / re-exports):
forbidden_dependency_check rule added:
Verification run (command + result):
cargo check --workspace --all-targets: pass/fail
forbidden_dependency_check: pass/fail
New risks:
Next step:
```

## 7. Verification Checklist

After every commit:

- `cargo check --workspace --all-targets` green (or `cargo check --all-targets` pre-Phase-B).
- No formatting command was run.
- No new `pub` items added unless explicitly justified.

After each sub-phase:

- Targeted tests for the touched module / cluster (`cargo test -p <crate> --lib` post-B).
- `who_imports` on any visibility change to confirm no external consumer broke.
- For cycle-break sub-phases (A.1, A.2): grep for the eliminated cross-edge — must return zero.

After each phase exit gate (A, B, C):

- `workspace_stats` — confirm `pub_crate_share` rises (or stays high).
- `dead_pub_in_crate` — confirm the unused-pub count is not growing.
- `forbidden_dependency_check` (post-B) — green against the per-phase rule set.
- Settle pass through at least one normal-feature-work change.

## 8. Success Criteria

Phase A success:

- No SCCs in the module graph (verified by grep + manual review).
- `graph/mod.rs` contains no glob `pub use` re-exports; every export is a named, reviewed entry.
- `pub_crate_share` for `graph` measurably higher than baseline.
- Codebase functionally unchanged; all examples + tests pass.

Phase B success:

- `crates/rmc-engine` and `crates/rmc-graph` exist as workspace members.
- `rmc-engine` is self-contained (no `use crate::` outside the engine cluster).
- `rmc-graph` depends only on `rmc-engine`.
- `forbidden_dependency_check` passes the §4.B.8 rule set.
- Main crate's `lib.rs` has facade re-exports keeping all in-repo consumer paths stable.

Phase C success:

- All structural code lives in workspace member crates.
- Main crate is binary + glue only (`src/main.rs` + `src/lib.rs` facade + `src/bin/`).
- `forbidden_dependency_check` passes the §5.C exit rule set.
- The repo can be reorganized for distribution: each member crate could in principle ship to crates.io as its own package without further refactoring.

## 9. Risk Summary

| Phase | Risk | Primary failure mode | Mitigation |
|---|---|---|---|
| A.1 | Low | Trait extraction is too narrow and forces widening | Use the smallest possible trait; expand only when actually needed |
| A.2 | Medium | Sweeping import rewrite misses a site | Grep gate before commit (`grep -rn 'tools::project_paths' src/` returns empty) |
| A.3 | Medium-high | Demoting a `pub` breaks an example / test consumer | `who_imports` check before each demotion; one cluster per commit |
| B.0–B.5 | Low | Path / re-export typos break builds | `cargo check --workspace` between commits |
| B.7 | Medium | Cross-crate import rewrite in `graph` misses a site | Stage as B.7.a.1 (move) + B.7.a.2 (rewrites); commits don't need to compile mid-flight as long as the next one does. Pre-close grep gate for `crate::{search,indexing,tools,mcp,config}` returning zero |
| B.8 | Low | Forbidden-dep rule set is wrong | Iterate the rule set; it's a config file |
| C.1 | Low | `rmc-config` is three small files; mostly a rename | `cargo check --workspace` after the import rewrite |
| C.2 | Medium-high | `indexing::unified`'s 11-target fan-out import rewrite; sequencing assumes C.1 already landed | Stage as C.2.a.1 (move all five modules) + C.2.a.2-4 (rewrites). Verify `Backup` trait from A.1 is in place before starting |
| C.3 | High | `tools` is the most-connected module; rewrite is largest | Multi-sub-commit staging is mandatory here |
| C.4 | Low | Main `lib.rs` facade misses a re-export and breaks `tests/` | `cargo check --workspace --all-targets` (the `--all-targets` is the gate) |

## 10. When to Abandon a Phase

- **Phase A.1/A.2** — abandon if the cycle is more entrenched than the analysis suggests; in that case the modules are misnamed and probably want to be merged instead of separated. Capture this as a different Phase 6.5 work item.
- **Phase A.3** — do not abandon based on count. The size of the contract isn't the problem; un-reviewed-ness is. If review confirms 60 of the 66 as real API, that's the answer; leave them `pub` and remove the glob. Abandon A.3 only if the review reveals that the right place to draw the API line *isn't* in `graph` at all (e.g. tool-response shapes belong in `tools`); in that case the work is "move types," not "narrow visibility," and belongs in a different plan.
- **Phase B** — abandon if `rmc-engine` cannot be made self-contained (i.e. some hidden inversion shows up that A.1/A.2 didn't catch). Stop and re-run Phase A on the new cycle.
- **Phase C** — abandon if no consumer of the lifted-out crates ever materializes. The cost of three crates with no external user is pure ceremony.

The default outcome of this plan is: **Phase A done, Phases B and C deferred indefinitely**. That is a successful outcome.

## 11. End-State Target Tree (after Phase C)

```text
rust-code-mcp-final/
  Cargo.toml                       # [workspace] members = [".", "crates/*"]
  src/
    main.rs                        # binary entry
    lib.rs                         # facade re-exports of all crates
    bin/
      test_tools_direct.rs

  crates/
    rmc-engine/
      Cargo.toml
      README.md
      src/
        lib.rs
        parser/         { mod.rs, types.rs, rust_parser.rs, call_graph.rs, imports.rs, type_references.rs }
        chunker/        { mod.rs, types.rs, chunker.rs, split.rs }
        embeddings/     { mod.rs, backend.rs, profile.rs, profile_registry.rs, batching.rs, util.rs, identity.rs, qwen3.rs, fastembed_cpu.rs, token_lengths.rs, error.rs, openrouter/* }
        vector_store/   { mod.rs, lancedb.rs, traits.rs, error.rs }
        search/         { mod.rs, bm25.rs, resilient.rs, rrf_tuner.rs, error.rs }
        schema.rs

    rmc-graph/
      Cargo.toml
      README.md
      src/
        lib.rs
        graph/          { mod.rs, ids.rs, model.rs, storage.rs, snapshot.rs, loader.rs, hir_trim.rs, ast_resolve.rs, extract.rs, bindings.rs, usages.rs, impls.rs, signatures.rs, attributes.rs, statics.rs, docs_audit.rs, derive_audit.rs, unsafe_audit.rs, fn_body_audit.rs, channel_audit.rs, recursion_check.rs, audit_util.rs, labels.rs, math.rs, embedding_cache.rs, test_support.rs, query/*, codemap/* }

    rmc-config/
      Cargo.toml
      README.md
      src/
        lib.rs
        config/         { mod.rs, indexer.rs, errors.rs }

    rmc-indexing/
      Cargo.toml
      README.md
      src/
        lib.rs
        indexing/       { mod.rs, unified.rs, unified_parallel.rs, indexer_core.rs, embedding_batcher.rs, file_processor.rs, incremental.rs, merkle.rs, tantivy_adapter.rs, consistency.rs, identity.rs, retry.rs, backup.rs, error.rs, error_collection.rs }
        monitoring/     { mod.rs, health.rs, backup.rs }
        metadata_cache/ { mod.rs }
        metrics/        { mod.rs, memory.rs }
        security/       { mod.rs, secrets.rs }

    rmc-server/
      Cargo.toml
      README.md
      src/
        lib.rs
        tools/          { mod.rs, router.rs, params/*, endpoints/*, graph/* }
        mcp/            { mod.rs, sync.rs, project_paths.rs }
        semantic/       { mod.rs, loader.rs, position.rs, rename.rs }
```

Notes:

- After Phase B (no C yet), the layout has just `rmc-engine` and `rmc-graph`; everything currently in `src/` other than the engine + graph modules stays in main crate.
- After Phase A only, no `crates/` directory exists; the change vs. PR-21 state is the two cycle breaks and the narrowed `graph` re-export list.
- `vendor/fastembed/` is never touched in any phase; it stays where it is.

## 12. What This Plan Deliberately Does NOT Do

- **Does not split `rmc-engine` into per-concern crates** (`rmc-parser`, `rmc-chunker`, …). The engine cluster has small declared surfaces (3–16 each) and tight internal coupling; one umbrella crate is the right granularity. If demand for sub-crate ships arises, that's a future plan.
- **Does not introduce a separate `rmc-core` for shared types**. There are no shared types across the proposed crates that aren't already in `rmc-engine`.
- **Does not version-publish anything**. The workspace is in-repo only. crates.io publishing is its own separate decision with separate concerns (semver discipline, MSRV policy, dependency pinning).
- **Does not change MCP tool names or param-struct external paths**. The §15 guardrail of the parent plan continues to hold — through facade re-exports in main `lib.rs`, all `rust_code_mcp::tools::…` paths resolve unchanged.
- **Does not touch `vendor/fastembed/`**. Out of scope, always.
- **Does not require Phase B or C to run.** Phase A standing alone is a valid successful outcome of this plan; B and C are optional escalations.
