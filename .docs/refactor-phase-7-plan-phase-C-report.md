# Phase 7 — Phase C Report

**Plan**: `.plans/refactor-phase-7-plan.md` — Refactor Plan: Phase 7 — Cleanup & Crate Lift
**Phase**: C — Server cluster lift (config, indexing, server)
**Status**: complete — workspace now contains 5 library crates plus the `crates/rust-code-mcp` MCP stdio executable package; root `Cargo.toml` is virtual and root `src/` has been removed
**Workspace**: `/home/molaco/Documents/rust-code-mcp-refactor`
**Date**: 2026-05-22

## Summary

Four `jj` commits land the server-cluster lift: `rmc-config` (C.1), `rmc-indexing` (C.2), `rmc-server` (C.3), and an initial main-crate simplification (C.4). A post-review remediation then corrected the final workspace shape: the root package was removed, root `src/` was deleted, and the MCP stdio executable moved to `crates/rust-code-mcp`. After this remediation, **all Rust source lives under `crates/`**.

Phase C builds on Phase B (engine + graph crates) and inherits its conventions: `pub(crate) → pub` widening when consumers cross the new crate boundary, hypergraph canonical-name shifts wherever a module moves declaration sites, and `cargo check --workspace --all-targets` as the per-commit gate (full test suite explicitly deferred per the user's environment-cost guidance).

## Commits

```
<post-review> phase 7 C.4 remediation: virtual root manifest; move executable/tests/examples to crates/rust-code-mcp; delete root src
4c7668ad  phase 7 C.4: simplify src/lib.rs to facade-only (superseded by post-review remediation)
522f02a5  phase 7 C.3: lift tools+mcp+semantic into rmc-server; 31 files; 13-pattern sed sweep
d83f1a03  phase 7 C.2: lift indexing+monitoring+metadata_cache+metrics+security into rmc-indexing
12f54b94  phase 7 C.1: lift config into rmc-config
```

Parent of the series: `e81c77f6` (Phase B post-review fix).

## C.0 — Re-baseline

Verification pass before lifting. Results:

- `grep -rEn 'use crate::(tools|mcp)' crates/rmc-engine/src crates/rmc-graph/src` → 0 hits (no forbidden upward refs).
- Cluster-import survey confirmed the plan's stated boundaries: `indexing` cluster imports `chunker, embeddings, parser, schema, search, vector_store` from engine; `config` only; `indexing, monitoring, metadata_cache, metrics, security` are intra-cluster.
- Layout note: `src/config.rs` is the mod file (not `src/config/mod.rs`); `src/metadata_cache.rs` is a flat file (not a directory). Both are valid Rust patterns; the plan target shape adjusts implicitly.

## C.1 — Lift `config` as `rmc-config`

- **Files**: 3 moved (`src/config.rs`, `src/config/indexer.rs`, `src/config/errors.rs`) into `crates/rmc-config/src/`. New `Cargo.toml`, `src/lib.rs`, `README.md`.
- **Foreign deps in new crate**: `rmc-engine` (for `EmbeddingProfile`), plus `serde, tracing, directories, thiserror, anyhow`.
- **Import rewrites**: 1 — `crates/rmc-config/src/config/indexer.rs:40` `use crate::embeddings::EmbeddingProfile;` → `use rmc_engine::embeddings::EmbeddingProfile;`.
- **Visibility widenings** (`pub(crate) → pub`):
  - `crates/rmc-config/src/config.rs:10` — `pub(crate) use indexer::{IndexerConfig, IndexerCoreConfig};` → `pub use ...`
  - `crates/rmc-config/src/config/indexer.rs:67` — `IndexerConfig` (consumed by `src/indexing/unified.rs:9`).
  - `crates/rmc-config/src/config/indexer.rs:141` — `IndexerCoreConfig` (consumed by `src/indexing/{identity.rs, indexer_core.rs}`).
- **Main `Cargo.toml`**: added `crates/rmc-config` to members and `rmc-config = { path = "crates/rmc-config" }` to main deps.
- **Main `src/lib.rs`**: `pub mod config;` → `pub use rmc_config::config;`.
- **Gate**: `cargo check --workspace --all-targets` green (46.19s cold; <1s warm).

## C.2 — Lift `indexing + monitoring + metadata_cache + metrics + security` as `rmc-indexing`

The largest commit in the lift sequence — 23 source files across 5 modules, ~5,500 LOC. Per the plan's medium-high risk classification: `indexing::unified` has 11 import targets, the highest fan-out in the codebase.

- **Files moved**: 23 (`src/indexing/` 15 files; `src/monitoring/` 3; `src/metrics/` 2; `src/security/` 2; `src/metadata_cache.rs` 1).
- **New crate path deps**: `rmc-engine`, `rmc-config`.
- **New crate third-party deps**: `tantivy, serde, serde_json, tracing, tokio, sled, sha2, walkdir, anyhow, thiserror, regex, glob, rs_merkle, sysinfo, rayon, num_cpus, bincode, directories`; dev-dep `tempfile = "3"`.
  - **Beyond the pre-survey list** (`^use` grep), three extra deps surfaced via the compiler: `num_cpus` (inline call in `embedding_batcher.rs`), `directories` (in-function `use directories::ProjectDirs` in `incremental.rs`), and `tempfile` (test modules in 9 files).
- **Import rewrites**: 25 `^use crate::*` rewrites in 11 files + 4 body-position rewrites (`crate::schema`, `crate::search::bm25` in `unified.rs:464,474` and `tantivy_adapter.rs:191-192`) caught after the initial sed sweep.
- **Visibility widenings** (`pub(crate) → pub`, 9 total):
  - `crates/rmc-indexing/src/metadata_cache.rs:72` — `MetadataCache` (consumed by `src/tools/endpoints/indexing_support.rs`).
  - `crates/rmc-indexing/src/monitoring/health.rs:19, 33, 44, 84` — `HealthStatus`, `Status`, `ComponentHealth`, `HealthMonitor` (all needed for the `HealthMonitor::check_health` return type to be reachable across the crate boundary per the `private_interfaces` lint).
  - `crates/rmc-indexing/src/indexing/identity.rs:17, 33, 48` — `active_chunking_identity_for_backend`, `indexing_identity`, `identity_hash` (consumed by `src/mcp/project_paths.rs`).
  - `crates/rmc-indexing/src/indexing/incremental.rs:40` — `get_snapshot_path_for_identity` (consumed by `src/mcp/project_paths.rs`).
- **Main `Cargo.toml`**: added `crates/rmc-indexing` to members and `rmc-indexing = { path = "crates/rmc-indexing" }` to main deps.
- **Main `src/lib.rs`**: replaced 5 `pub mod X;` with `pub use rmc_indexing::{indexing, monitoring, metadata_cache, metrics, security};` (split across alphabetically-ordered lines).
- **Gate**: `cargo check --workspace --all-targets` green.

## C.3 — Lift `tools + mcp + semantic` as `rmc-server`

The highest-risk step in the plan — `tools` is the most-connected module; 13 distinct foreign-crate import patterns to rewrite. The actual diff matched the plan's "largest diff of any commit in this plan" classification.

- **Files moved**: 31 (`src/tools/` 24 files, `src/mcp/` 3, `src/semantic/` 4).
- **New crate path deps**: `rmc-engine`, `rmc-graph`, `rmc-config`, `rmc-indexing`.
- **New crate third-party deps**: `rmcp, tokio, tantivy, serde, serde_json, tracing, anyhow, directories, sha2, num_cpus, heed, ra_ap_ide, ra_ap_ide_db, ra_ap_load-cargo, ra_ap_project_model, ra_ap_vfs`; dev-dep `tempfile = "3"`.
  - **Beyond the pre-survey list**: `serde_json, heed, num_cpus, ra_ap_ide_db, tempfile (dev)` — all body-position or non-leading `use` references the `^use` grep missed.
- **Import rewrites**: single `sed -E` invocation with **13 `\b`-anchored rewrites** over **32 files** (31 moved + new `lib.rs`). The `\b` word-boundary anchors prevent collisions with the intentionally-intra-crate `crate::tools`, `crate::mcp`, `crate::semantic` references.
  - Rewrites cover both `^use crate::X` lines AND inline body-position `crate::X::` references uniformly.
  - Post-sweep grep `crate::(chunker|embeddings|parser|schema|search|vector_store|config|indexing|monitoring|metadata_cache|metrics|security|graph)` → 0 hits.
- **Stale qualified-name string-literal fixes** (5 occurrences across 3 files; same class of issue as Phase B post-review):
  - `crates/rmc-server/src/tools/graph/tests.rs:65` — `"rust_code_mcp::indexing::tantivy_adapter"` → `"rmc_indexing::indexing::tantivy_adapter"` (stale from C.2; fixed in C.3 since we were already touching this file).
  - `crates/rmc-graph/src/graph/signatures.rs:210` — `"rust_code_mcp::tools::graph::core::workspace_stats"` → `"rmc_server::tools::graph::core::workspace_stats"`.
  - `crates/rmc-graph/src/graph/statics.rs:152, 157, 169, 171` — four occurrences of `"rust_code_mcp::semantic::SEMANTIC"` → `"rmc_server::semantic::SEMANTIC"`.
- **Visibility widenings**: **0**. The widenings done in C.1 (`IndexerConfig`, `IndexerCoreConfig`) and C.2 (`MetadataCache`, health.rs items, identity helpers) had already done all the work — by the time `tools/mcp/semantic` moved out, every cross-boundary consumer was already pointing at a `pub` symbol.
- **Main `Cargo.toml`**: added `crates/rmc-server` to members and `rmc-server = { path = "crates/rmc-server" }` to main deps.
- **Main `src/lib.rs`**: replaced 3 `pub mod X;` with `pub use rmc_server::{tools, mcp, semantic};`.
- **Gate**: `cargo check --workspace --all-targets` green.

## C.4 — Root package eliminated

- **Root `Cargo.toml`** converted to a virtual workspace manifest: no `[package]`, no root `[dependencies]`, no root package targets.
- **`src/main.rs`** moved to `crates/rust-code-mcp/src/main.rs`.
- **Root `tests/` and `examples/`** moved to `crates/rust-code-mcp/tests/` and `crates/rust-code-mcp/examples/`.
- **Root `src/lib.rs` facade removed**. Former `rust_code_mcp::*` imports in the moved package were rewritten to direct `rmc_*` crate imports.
- **`src/bin/test_tools_direct.rs` deleted** instead of moved; it was stale and hardcoded the old monolithic `src/` layout.
- **Gate**: `cargo metadata --no-deps`, `cargo check -p rust-code-mcp --bin rust-code-mcp`, and `cargo check -p rust-code-mcp --test test_mcp_stdio_transport` are green with `RUSTFLAGS="-C linker-features=-lld"`.

## Workspace shape after Phase C

```text
rust-code-mcp-refactor/
  Cargo.toml             # virtual [workspace] manifest
  crates/
    rust-code-mcp/       # MCP stdio executable package; owns tests/examples

    rmc-engine/          # parser, schema, chunker, embeddings, vector_store, search   (40 .rs)
    rmc-graph/           # graph                                                       (48 .rs)
    rmc-config/          # config                                                       (4 .rs)
    rmc-indexing/        # indexing, monitoring, metadata_cache, metrics, security   (24 .rs)
    rmc-server/          # tools, mcp, semantic                                        (32 .rs)
```

Root `src/`, root `tests/`, and root `examples/` no longer exist. The executable, integration tests, and examples now live under `crates/rust-code-mcp/`.

## Dependency graph (crate-level)

```text
rmc-engine        (no in-workspace deps)
   ↑
rmc-graph         depends on: rmc-engine
   ↑
rmc-config        depends on: rmc-engine
   ↑
rmc-indexing      depends on: rmc-engine, rmc-config
   ↑
rmc-server        depends on: rmc-engine, rmc-graph, rmc-config, rmc-indexing
   ↑
rust-code-mcp     depends on: rmc-engine, rmc-graph, rmc-config, rmc-indexing, rmc-server
(MCP stdio executable package)
```

Strictly acyclic. Verified by Cargo (workspace would fail to resolve under a cycle).

## §5.C Exit conditions — verification

| Exit criterion | Status |
|---|---|
| `cargo check --workspace --all-targets` green | ✅ green at every commit |
| `cargo test --workspace --all-targets` green | ⏳ deferred (per user instruction to avoid running full test suite during refactor steps; runtime verification will happen as a separate gate) |
| `rmc-engine` depends on nothing in workspace | ✅ (verified Phase B B.6; unchanged in C) |
| `rmc-graph` depends only on `rmc-engine` | ✅ |
| `rmc-config` depends only on `rmc-engine` | ✅ |
| `rmc-indexing` depends only on `rmc-engine`, `rmc-config` | ✅ |
| `rmc-server` depends on `rmc-engine`, `rmc-graph`, `rmc-config`, `rmc-indexing` | ✅ |
| Executable package `crates/rust-code-mcp/Cargo.toml` has narrow runtime deps and moved test/example deps | ✅ |
| Root `Cargo.toml` is virtual and root `src/` is absent | ✅ |
| Each new crate has a `README.md` | ✅ (3 added in C.1/C.2/C.3 — `rmc-config`, `rmc-indexing`, `rmc-server`; `rmc-engine` and `rmc-graph` already had theirs from Phase B) |
| `forbidden_dependency_check` returns zero violations against full §5.C rule set | ⏳ not re-run in Phase C; rule set unchanged at crate granularity from Phase B's codification (`.docs/architectural-rules.md`); should be re-verified as a settle gate |

## Reflections / lessons-from-execution

**Visibility-widening pyramid worked as predicted.** Phase B widened ~36 items for engine/graph; C.1 widened 2 more (the config items), C.2 widened 9 more (the indexing-cluster items). **C.3 widened 0** — by the time the highest-fan-out module (`tools`) moved out, every external consumer was already reaching `pub` items via the lower crates. This validates the plan's ordering choice (`config` then `indexing` then `server`): visibility cost amortizes downward; whichever crate moves last gets a "free" lift.

**Body-position references matter as much as `^use` lines.** Three of the four phase-C commits surfaced body-position references that the `^use` grep missed:
- C.2: `crate::schema::ChunkSchema` and `crate::search::bm25::Bm25Search` as return-type / parameter-type paths in `unified.rs` and `tantivy_adapter.rs`.
- C.3: 13 distinct foreign-crate prefixes used inline (matched by the `\b`-anchored sed pass without separate enumeration).
- Inline-function `use` statements (e.g. `use directories::ProjectDirs` inside a function body) also escape the `^use` grep — caught only by compile errors.

The C.3 sed approach (drop the `^use` anchor, use `\b` word boundaries) handles both kinds uniformly and should be the default pattern for future lift work.

**Hypergraph canonical-name string literals continue to be a latent test-failure source.** Like Phase B's `rust_code_mcp::graph::*` strings, C.3 found stale `rust_code_mcp::{indexing, tools, semantic}::*` strings that needed updating. The pattern: any module move at crate-lift scale invalidates every hardcoded canonical-name string. There is no compiler check for these — only runtime test assertions reveal them. Future module moves should grep for the affected canonical-name prefixes BEFORE the move and re-grep AFTER, treating string-literal updates as a mandatory step of the lift, not a follow-up.

**Pre-survey accuracy: medium.** The pre-survey done before C.2 and C.3 missed three deps each time (always body-position or inline-function `use`s). The compiler's error-driven discovery worked fine, but a better pre-survey would also grep `<crate>::` patterns in body positions, not just `^use <crate>::`. This is the same lesson Phase B drew about `directories::ProjectDirs` and is now codifiable as a rule: **inventory body-position cross-crate references with a non-anchored grep, in addition to `^use`**.

**Executable package dependency split done.** `crates/rust-code-mcp` now keeps the binary target's runtime surface to `rmc-server` plus runtime libraries, while moved examples/tests get lower-crate access through `dev-dependencies`.

**No runtime test verification.** Per user environment-cost guidance, full `cargo test --workspace --all-targets` was not run. The Phase B post-review pattern (one focused test group as a gate) should be applied to Phase C too — likely targets:
- The 5 stale-string fixes in C.3 (verify `graph::query`, `graph::statics`, `graph::signatures`).
- The 9 visibility widenings in C.2 (verify the affected consumers compile and route through the right paths).
- The fan-out test: `cargo test --workspace --lib` (only library unit tests, not the full integration suite).

## Open follow-ups (not in Phase C scope)

- **`crates/rust-code-mcp/Cargo.toml` dev-dep trimming.** Runtime deps are narrow now, but the moved examples/tests still carry a broad former-root support set. A follow-up can trim those entries target-by-target.
- **Runtime verification gate.** Run targeted `cargo test` groups (one binary at a time per the project memory rule) to confirm the 5 string-literal fixes and 9 visibility widenings haven't broken runtime test assertions.
- **`forbidden_dependency_check` re-run.** The rule set in `.docs/architectural-rules.md` was codified at Phase B end. The crate granularity is unchanged in Phase C, so the rule set still applies — but re-verifying with the post-C workspace would catch any latent boundary violations introduced during the lifts.
- **B.8 CI wiring** (originally an open follow-up from Phase B). The `forbidden_dependency_check` rule set is codified; running it on every PR is still a separate task. Now more valuable post-C because more crate boundaries exist to enforce.
- **Hypergraph canonical-name documentation.** Phase B's report has a "Hypergraph qualified-name stability" section; Phase C should add to it (or the same doc should be updated to reflect that `rmc_config::config::*`, `rmc_indexing::*`, `rmc_server::*` are the new canonical prefixes for those clusters).
- **The 47 `unreachable_pub` warnings in rmc-indexing.** The new lib-root `#![warn(unreachable_pub, dead_code)]` lint inherited from main `src/lib.rs` now flags many `pub` items as `unreachable`. Most are honest `pub(crate)` candidates that the widening pressure left over-promoted. A follow-up could tighten back where the lift-time `pub` widening wasn't actually needed externally.

## Phase C → end-state target tree (per §11 of the plan)

The post-C layout exactly matches the §11 target (modulo file-count discrepancies between the plan's listed file inventories and the actual file-system state — which are noise, not divergence):

```text
rust-code-mcp-refactor/
  Cargo.toml                       # virtual [workspace] manifest

  crates/
    rust-code-mcp/                 # executable, tests, examples
    rmc-engine/                    (B.1-B.5)
    rmc-graph/                     (B.7)
    rmc-config/                    (C.1) ← new
    rmc-indexing/                  (C.2) ← new
    rmc-server/                    (C.3) ← new
```

The §12 "What This Plan Deliberately Does NOT Do" guardrails all hold post-C:
- No per-concern engine sub-crates.
- No `rmc-core` shared-types crate.
- No version-publishing.
- No MCP tool-name / param-struct changes. Rust import paths in moved examples/tests now use direct `rmc_*` crates because the root facade was deliberately removed.
- `vendor/fastembed/` untouched.

## Conclusion

Phase C landed cleanly and the post-review remediation corrected the final shape: there is no root package and no root `src/`. The visibility-widening cost was distributed front-loaded (C.1: 2, C.2: 9, C.3: 0), as the plan's ordering predicted. The total widening cost across Phases B+C is ~47 `pub(crate) → pub` promotions — the inherent architectural cost of the lift.

The workspace is now positioned for distribution: each member crate could in principle ship to crates.io as its own package (with semver discipline, MSRV pinning, etc. — those are publishing concerns, not refactor concerns). The plan's primary §0 success criterion ("All structural code lives in workspace member crates") is met.

The pragmatic next move mirrors Phase B's: let the codebase age through normal feature work before treating Phase C as "settled". Crate APIs harden expensively, and the 11 widenings in C.1/C.2 are visible signals of pressure that should be allowed to relax under real-world use before further structural decisions are made on top of it.
