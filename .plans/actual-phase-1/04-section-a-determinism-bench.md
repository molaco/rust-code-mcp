# Section A — P0.1 Determinism + P0.4 Benchmark Pool

## Overview

This slice de-risks the two non-engine items on the M0 critical path. P0.1 makes the cold build of a workspace reproducible byte-for-byte (or content-for-content) so every downstream layer — the warm-host incremental writer (P0.2), the differential apply-vs-cold-rebuild test that gates every CRUD op (P1.5), the secondary-index diff-patch correctness check (issue #5), and the reward signal stability — has a stable ground truth to diff against. Without P0.1, the entire RL training signal can drift on iteration-order noise alone. P0.4 fetches and pins a 50-100 crate benchmark pool that builds cleanly under the nix devshell so the M0 feasibility spikes have realistic workspaces to run against.

These two items have no compile-order dependency on each other, but they share a tester. The reproducibility test (P0.1) needs at minimum one tiny crate to exercise the byte-equality check, and the pool (P0.4) needs the build pipeline to ingest each member without panicking — so the P0.1 implementation should be wired such that the pool members themselves become continuous regression fixtures for determinism. Both must land before M0's two feasibility spikes start.

## Existing nondeterminism inventory (audited)

The extract → persist pipeline has these confirmed `HashMap`/`HashSet` iteration sites whose iteration order propagates into either `Vec`-ordering on `ExtractionModel` or DUP_SORT secondary-index insert-order in LMDB:

1. `crates/rmc-graph/src/graph/bindings.rs:54` — `for (module_id, _) in def_map.modules()` (RA-internal; the outer loop iterates `local_crates` which is ordered).
2. `crates/rmc-graph/src/graph/bindings.rs:116` — `let mut seen: HashSet<(NodeId, String, NodeId, BindingKind)>` dedups `model.bindings` via `retain`. `retain` preserves Vec order — fine.
3. `crates/rmc-graph/src/graph/usages.rs:45` — `for (&def_id, &target_node_id) in def_to_node`. **The worst offender**: `model.usages` is built in HashMap-iteration order.
4. `crates/rmc-graph/src/graph/signatures.rs:47` — same pattern; `model.signatures` order is HashMap order.
5. `crates/rmc-graph/src/graph/statics.rs:33` — same; `model.statics` is HashMap order.
6. `crates/rmc-graph/src/graph/snapshot.rs:408-456` — `write_model` writes DUP_SORT secondaries. **DUP_SORT stores duplicates in value-sort-order**, so insert order does not affect on-disk byte layout for the duplicate values themselves — primary tables are content-addressed and stored by key, also fine.

**Conclusion on byte-equality:** The LMDB file content is determined by `{key, value}` set union — primary tables keyed on content-addressed IDs, secondary DUP_SORT keyed on NodeId with content-addressed values, all sorted. The on-disk layout *should* be deterministic given the same input set, except for LMDB free-list / page-allocation noise. We therefore target **content-equality** (set-equality after `mdb_dump`-style iteration) as the primary contract and **byte-equality** (after `mdb_copy --compact`) as a strict-mode bonus.

## New modules / files

- `crates/rmc-graph/src/graph/determinism.rs` — new module. Houses the canonical sort orders for `ExtractionModel.bindings`, `usages`, `contains`, `signatures`, `statics`; the public `sort_model_for_persistence(&mut ExtractionModel)` entrypoint called from `extract::extract` before `write_model`.
- `crates/rmc-graph/src/graph/snapshot_compare.rs` — new module. Defines the typed `SnapshotDumpError` (`thiserror`, wraps `heed::Error`). Public functions: `dump_snapshot(&OpenedSnapshot) -> Result<SnapshotDump, SnapshotDumpError>` returning a canonical in-memory representation of every sub-DB as `BTreeMap<Vec<u8>, BTreeSet<Vec<u8>>>`, and `compare_snapshots(&SnapshotDump, &SnapshotDump) -> SnapshotDiff` returning per-table set differences. Used by the golden test and later by P0.2's apply-vs-cold-rebuild differential test.
- `crates/rmc-graph/tests/determinism_golden.rs` — integration test. Builds the rmc workspace twice (two staging dirs), dumps both, asserts `compare_snapshots == empty`.
- `crates/rmc-graph/benches/determinism_bench.rs` — micro-benchmark to track the cost of `sort_model_for_persistence` (target: < 5% of total extract time).
- `bench/Cargo.toml` — new workspace **outside** the main workspace (path: `/home/molaco/Documents/rust-code-mcp-refactor/bench/Cargo.toml`). NOT a member of the rmc workspace. It is a separate Cargo workspace that vendors the 50-100 corpus crates.
- `bench/fetch_corpus.sh` — fetch / pin / verify-build script.
- `bench/corpus.toml` — declarative manifest: list of `[corpus.<slug>] git, rev, path, edition, expected_loc, tags`.
- `bench/README.md` — selection criteria, expected build time, troubleshooting.
- `crates/rmc-config/src/config.rs` (edit) — add a `Seed(u64)` newtype (private inner field + `new`/`value` accessors) and a `pub seed: Seed` field to `Config`, with env-var loader `RMC_SEED` (default `Seed::default()` == 0). `from_env` returns `Result<Self, ConfigError>` and surfaces a malformed `RMC_SEED` as `ConfigError::InvalidSeed`.
- `crates/rmc-graph/src/graph/snapshot.rs` (edit) — extend `BuildOptions` with `pub seed: Seed` (re-exporting `rmc_config::Seed`); thread through `extract::extract`.

## Type definitions

```rust
// crates/rmc-graph/src/graph/determinism.rs

pub(crate) fn sort_model_for_persistence(model: &mut ExtractionModel) {
    sort_contains(&mut model.contains);
    sort_bindings(&mut model.bindings);
    sort_usages(&mut model.usages);
    sort_signatures(&mut model.signatures);
    sort_statics(&mut model.statics);
}

fn sort_contains(v: &mut Vec<(NodeId, NodeId)>) {
    v.sort_unstable_by(|(p1, c1), (p2, c2)| {
        p1.as_bytes().cmp(p2.as_bytes()).then_with(|| c1.as_bytes().cmp(c2.as_bytes()))
    });
    v.dedup();
}

fn sort_bindings(v: &mut Vec<Binding>) {
    v.sort_unstable_by(|a, b| {
        super::snapshot::binding_id_for(a).as_bytes().cmp(
            super::snapshot::binding_id_for(b).as_bytes(),
        )
    });
}

fn sort_usages(v: &mut Vec<Usage>) {
    v.sort_unstable_by(|a, b| {
        super::snapshot::usage_id_for(a).as_bytes().cmp(
            super::snapshot::usage_id_for(b).as_bytes(),
        )
    });
}

fn sort_signatures(v: &mut Vec<(NodeId, FunctionSignature)>) {
    v.sort_unstable_by(|(a, _), (b, _)| a.as_bytes().cmp(b.as_bytes()));
}

fn sort_statics(v: &mut Vec<(NodeId, StaticMetadata)>) {
    v.sort_unstable_by(|(a, _), (b, _)| a.as_bytes().cmp(b.as_bytes()));
}
```

```rust
// crates/rmc-graph/src/graph/snapshot_compare.rs

/// Failure while dumping an opened snapshot into its canonical in-memory form.
///
/// Wraps the underlying heed/LMDB read failure so callers can distinguish a
/// transient storage fault from a content mismatch reported by
/// `compare_snapshots`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SnapshotDumpError {
    /// A read transaction or cursor iteration over a sub-DB failed.
    #[error("failed to read snapshot sub-DB during dump")]
    Read(#[from] heed::Error),
}

/// Canonical in-memory image of every persisted sub-DB, used for content-equality
/// comparison of two cold builds. Fields are private; growable, so non-exhaustive.
/// Construct only via `dump_snapshot`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SnapshotDump {
    nodes: BTreeMap<Vec<u8>, Vec<u8>>,             // bincode-encoded Node
    bindings: BTreeMap<Vec<u8>, Vec<u8>>,
    bindings_by_from_module: BTreeMap<Vec<u8>, BTreeSet<Vec<u8>>>,
    bindings_by_target: BTreeMap<Vec<u8>, BTreeSet<Vec<u8>>>,
    children_by_parent: BTreeMap<Vec<u8>, BTreeSet<Vec<u8>>>,
    usages: BTreeMap<Vec<u8>, Vec<u8>>,
    usages_by_target: BTreeMap<Vec<u8>, BTreeSet<Vec<u8>>>,
    usages_by_consumer: BTreeMap<Vec<u8>, BTreeSet<Vec<u8>>>,
    usages_by_consumer_function: BTreeMap<Vec<u8>, BTreeSet<Vec<u8>>>,
    signatures: BTreeMap<Vec<u8>, Vec<u8>>,
    statics: BTreeMap<Vec<u8>, Vec<u8>>,
    meta: BTreeMap<String, Vec<u8>>,               // excludes "graph_id", "created_at_unix"
}

impl SnapshotDump {
    /// Primary `nodes` table: content-addressed NodeId bytes → bincode `Node`.
    #[must_use]
    pub fn nodes(&self) -> &BTreeMap<Vec<u8>, Vec<u8>> { &self.nodes }
    /// Primary `bindings` table.
    #[must_use]
    pub fn bindings(&self) -> &BTreeMap<Vec<u8>, Vec<u8>> { &self.bindings }
    /// Primary `usages` table.
    #[must_use]
    pub fn usages(&self) -> &BTreeMap<Vec<u8>, Vec<u8>> { &self.usages }
    /// Primary `signatures` table.
    #[must_use]
    pub fn signatures(&self) -> &BTreeMap<Vec<u8>, Vec<u8>> { &self.signatures }
    /// Primary `statics` table.
    #[must_use]
    pub fn statics(&self) -> &BTreeMap<Vec<u8>, Vec<u8>> { &self.statics }
    /// Metadata table (excludes `"graph_id"` and `"created_at_unix"`).
    #[must_use]
    pub fn meta(&self) -> &BTreeMap<String, Vec<u8>> { &self.meta }
    // Remaining DUP_SORT secondaries (`*_by_*`) exposed analogously as needed.
}

/// Per-table set differences between two dumps. Private fields; non-exhaustive.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct SnapshotDiff { /* per-table _only_in_a / _only_in_b / _value_differs */ }

impl SnapshotDiff {
    /// True when every per-table difference set is empty (content-equal).
    #[must_use]
    pub fn is_empty(&self) -> bool { /* all empty */ }
    /// Keys whose `nodes` value differs between the two dumps.
    #[must_use]
    pub fn nodes_value_differs(&self) -> &BTreeSet<Vec<u8>> { /* accessor */ }
}

/// Dump every sub-DB of an opened snapshot into its canonical in-memory form.
///
/// # Errors
/// Returns [`SnapshotDumpError::Read`] if a read transaction or cursor iteration
/// over any sub-DB fails (preserving the underlying [`heed::Error`]).
pub fn dump_snapshot(snap: &OpenedSnapshot) -> Result<SnapshotDump, SnapshotDumpError>;

/// Compute per-table set differences. Pure; never fails.
#[must_use]
pub fn compare_snapshots(a: &SnapshotDump, b: &SnapshotDump) -> SnapshotDiff;
```

```rust
// crates/rmc-config/src/config.rs — Seed newtype + extend Config

/// Global determinism seed. Threaded through every stochastic step (future
/// clustering / GMM / node2vec consumers). Private inner field; `0` by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Seed(u64);

impl Seed {
    /// Construct a seed from a raw `u64`.
    #[must_use]
    pub fn new(value: u64) -> Self { Self(value) }
    /// The raw seed value.
    #[must_use]
    pub fn value(self) -> u64 { self.0 }
}

/// Malformed configuration drawn from the environment.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// `RMC_SEED` was set but did not parse as a `u64`.
    #[error("RMC_SEED is not a valid u64: {value:?}")]
    InvalidSeed {
        /// The offending raw value.
        value: String,
        /// The underlying parse failure.
        #[source]
        source: std::num::ParseIntError,
    },
}

pub struct Config {
    pub server_port: u16,
    pub data_dir: PathBuf,
    pub max_file_size: u64,
    pub num_threads: usize,
    pub debug: bool,
    pub retry_attempts: u32,
    pub retry_delay_ms: u64,
    /// Global determinism seed. See [`Seed`].
    pub seed: Seed,
}

impl Config {
    /// Build a `Config` from environment variables.
    ///
    /// # Errors
    /// Returns [`ConfigError::InvalidSeed`] if `RMC_SEED` is set but does not
    /// parse as a `u64`. A malformed seed is surfaced rather than silently
    /// defaulting to `0`, so determinism runs cannot drift on a typo'd env var.
    pub fn from_env() -> Result<Self, ConfigError> {
        let seed = match std::env::var("RMC_SEED") {
            Ok(raw) => Seed::new(raw.parse().map_err(|source| {
                ConfigError::InvalidSeed { value: raw, source }
            })?),
            Err(_) => Seed::default(), // unset / non-UTF-8 → default 0
        };
        Ok(Self {
            // ...existing fields...
            seed,
        })
    }
}
```

```rust
// crates/rmc-graph/src/graph/snapshot.rs — extend BuildOptions

pub struct BuildOptions {
    pub force_rebuild: bool,
    pub data_dir_override: Option<PathBuf>,
    pub env: GraphEnvOptions,
    /// Determinism seed (re-exported `rmc_config::Seed`). `Seed::default()` == 0.
    pub seed: Seed,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self { /* existing */ seed: Seed::default() }
    }
}
```

```toml
# bench/corpus.toml — declarative pool (excerpt)

[meta]
total_crates_target = 75
total_loc_target_min = 200_000

[corpus.serde_json]
git = "https://github.com/serde-rs/json"
rev = "v1.0.128"           # pinned tag, NOT a branch
path = "vendor/serde_json"
edition = "2021"
expected_loc = 18_000
tags = ["small", "serde", "no-build-rs", "no-proc-macro"]
build_cmd = "cargo check --offline --frozen --all-targets"
```

## Step-by-step implementation

### Phase 1: P0.1 — sort the extraction model

1. **WHAT**: Create `crates/rmc-graph/src/graph/determinism.rs` with `sort_model_for_persistence` and its five helpers. **DEPENDS**: nothing. **VERIFY**: `cargo build -p rmc-graph` succeeds.
2. **WHAT**: Add `mod determinism;` to `crates/rmc-graph/src/graph/mod.rs` after `pub(crate) mod snapshot;`. **DEPENDS**: 1. **VERIFY**: `cargo check -p rmc-graph` succeeds.
3. **WHAT**: In `snapshot.rs`, change `binding_id_for` / `usage_id_for` visibility from `pub(crate)` to `pub(in crate::graph)`. **VERIFY**: build succeeds.
4. **WHAT**: Call `determinism::sort_model_for_persistence(&mut model)` at the end of `extract::extract` (after `extract_usages`). **VERIFY**: existing extract tests still pass.
5. **WHAT**: Add the `Seed(u64)` newtype to `rmc-config`; add `pub seed: Seed` to `BuildOptions` (with `Default::default`) and to `Config` whose `from_env` returns `Result<Self, ConfigError>` and surfaces a malformed `RMC_SEED` as `ConfigError::InvalidSeed`. **VERIFY**: `BuildOptions::default().seed == Seed::default()` and `BuildOptions::default().seed.value() == 0`.
6. **WHAT**: Thread `seed: Seed` from `Config` → call sites that construct `BuildOptions`. Callers of `Config::from_env` now propagate/report the `Result<_, ConfigError>` (the binary entrypoint surfaces `ConfigError::InvalidSeed` at startup rather than running with a silent default). **VERIFY**: `cargo check --workspace` succeeds.
7. **WHAT**: Change `extract::extract(loaded: &LoadedWorkspace, seed: Seed) -> ExtractionModel`; thread seed through `sort_model_for_persistence` (today ignored — plumbing for P1.3 clustering). **VERIFY**: build.

### Phase 2: P0.1 — snapshot comparison + golden test

8. **WHAT**: Create `snapshot_compare.rs` with `SnapshotDump`, `SnapshotDiff`, `dump_snapshot`, `compare_snapshots`. For `meta_by_key` exclude `"graph_id"` and `"created_at_unix"`. **DEPENDS**: 1. **VERIFY**: unit test on a synthetic snapshot via `persist_test_model` round-trips to a non-empty dump.
9. **WHAT**: Create `crates/rmc-graph/tests/determinism_golden.rs` with `two_cold_builds_are_content_equal`. **DEPENDS**: 4 + 8. **VERIFY**: `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --test determinism_golden`. Expected ~30-60s.
10. **WHAT**: Add `#[ignore] byte_equal` variant using `heed::EnvOpenOptions::copy_to_file` with compaction. **VERIFY**: `cargo test -p rmc-graph --test determinism_golden -- --ignored` reports equal hashes.

### Phase 3: P0.1 — fix remaining ordering escapes

11–16. Audit `usages.rs:45`, `signatures.rs:47`, `statics.rs:33`, `bindings.rs:54`, `bindings.rs:42-44`, `impls.rs:42,49`. Convert HashMaps that are iterated for emission to `BTreeMap` (an `FxHashMap` is deterministic only with a fixed hash seed and build, which is too fragile to rely on for an on-disk ordering contract; `BTreeMap` gives key order by construction). Add `// HashMap-iteration: order leaks; canonicalized by graph::determinism::sort_*` comments. **VERIFY**: golden test from step 9 still passes.

### Phase 4: P0.4 — benchmark pool

17. **WHAT**: Create directory `bench/` (sibling of `crates/`) with `Cargo.toml`:
    ```toml
    [workspace]
    resolver = "3"
    members = []
    exclude = ["vendor"]
    ```
    **VERIFY**: `cargo check --manifest-path bench/Cargo.toml` succeeds.
18. **WHAT**: Add `bench/` to root `Cargo.toml`'s `[workspace] exclude`. **VERIFY**: workspace check does not visit `bench/`.
19. **WHAT**: Author `bench/corpus.toml` with 75 entries: 5 small smoke, 30 small, 25 medium, 10 large, 5 huge. Hard filter: must build with `cargo check --offline --frozen --all-targets` after `cargo fetch`; reject system-C-lib deps, build.rs network downloads, nightly-only crates. **VERIFY**: parses; entry count ≥ 50; every `rev` is a SHA or vX.Y.Z tag.
20. **WHAT**: Author `bench/fetch_corpus.sh`. Parse corpus.toml, clone-if-missing into `$VENDOR/<slug>`, checkout rev, update `bench/Cargo.toml` members, `cargo fetch --locked`, per-entry `cargo check`, record success/duration into `bench/build_report.json`. Exit 0 if ≥ 50 passed. **VERIFY**: `nix develop ../nix-devshells#cuda-code --command bash bench/fetch_corpus.sh` writes report with ≥ 50 passing.
21. **WHAT**: Author `bench/build_report.json` schema validator. **VERIFY**: known-good exits 0; passed < 50 exits 1.
22. **WHAT**: Smoke test `bench/tests/smoke.sh` that picks 3 smallest crates and runs `build_hypergraph`. **VERIFY**: produces non-empty `workspace_stats`.
23. **WHAT**: Extend `tests/determinism_golden.rs` with `#[ignore] corpus_crates_are_content_equal` over 5 smallest corpus crates. **VERIFY**: `RMC_BENCH_DETERMINISM=1 cargo test --ignored corpus_crates_are_content_equal`.

### Phase 5: integration + docs

24–26. Surface `seed` in `Config::print_summary`. Add "Determinism" and "Benchmark Pool" subsections to `AGENTS.md`.

## Tests

- **`two_cold_builds_are_content_equal`** — build rmc workspace twice into tmpdirs, dump, assert `SnapshotDiff::is_empty()`.
- **`two_cold_builds_are_byte_equal_after_compact`** (#[ignore]) — compact both LMDB envs and `sha256` `data.mdb`.
- **`corpus_crates_are_content_equal`** (#[ignore], `RMC_BENCH_DETERMINISM=1`) — over 5 smallest corpus crates.
- **`seed_threads_into_persisted_model`** — build with `BuildOptions { seed: Seed::new(42), .. }`, then read the persisted snapshot's `meta` (or the `BuildOptions` recorded by `extract::extract`) and assert the stored/threaded seed value is exactly `42` — i.e. the seed actually reaches the model/persist layer, not merely "no panic".
- **`from_env_rejects_malformed_seed`** — set `RMC_SEED=not-a-number`, assert `Config::from_env()` returns `Err(ConfigError::InvalidSeed { .. })`; set `RMC_SEED=7`, assert `Ok(cfg)` with `cfg.seed.value() == 7`; unset, assert `cfg.seed == Seed::default()`.
- **`dump_snapshot_surfaces_read_error`** — point `dump_snapshot` at a closed/corrupt env (or a sub-DB whose read txn fails) and assert it returns `Err(SnapshotDumpError::Read(_))` rather than panicking, exercising the failure path.
- **`sort_bindings_is_total_and_idempotent`** — shuffled IDs, two calls same result, two shuffles same outputs.
- **`sort_usages_is_total_and_idempotent`** — same shape for Usage.
- **`sort_contains_dedups`** — `[(A,B),(A,B),(C,D)]` → `[(A,B),(C,D)]`.
- **`dump_round_trip`** — `persist_test_model` with a known node, dump, assert the dump's `nodes()` contains that node's content-addressed key and the decoded value round-trips equal to the input (not merely "non-empty").
- **`compare_identical_dumps_is_empty`** — two read txns, same snapshot.
- **`compare_detects_node_diff`** — mutate one byte, assert `diff.nodes_value_differs().len() == 1`.

## Open decisions / risks

- **Risk: `def_map.modules()` RA-internal-stable**. If RA upgrades change ordering, the sort-after-extract neutralizes it for persist, but `def_to_node` insertion order could shift which Node value wins on a duplicate. *Mitigation*: assert all duplicates produce equal Node records.
- **Risk: byte-equality after `mdb_copy --compact` may differ due to LMDB metadata pages**. *Mitigation*: accept content-equality as primary contract; document strict mode as informational.
- **Risk: 50/75 corpus crates failing**. *Mitigation*: oversample (target 100 candidates, ship 50-75).
- **Risk: 75 git repos at full history is slow** (~5 GB). *Mitigation*: `git clone --depth 1 --branch <tag>`.
- **Risk: nix devshell pinned MSRV ≠ corpus MSRV**. *Mitigation*: pick corpus by MSRV ≤ devshell's stable channel.
- **Open decision: `bench/` as git submodule vs sibling repo?** Recommend submodule.
- **Open decision: where does seed get consumed in P0.1?** Today nowhere; it's plumbing for P1.3.
- **Risk: warm-host incremental rebuilds produce extraction in different order than cold rebuilds**. *Mitigation*: warm-host merge path (P0.2) must call same `sort_model_for_persistence` on merged model. Document this now.


---

