# Section B — M0 Contracts (D1–D4) + Feasibility Spikes

## Overview

This slice formalises the four written contracts that gate every subsequent build step (P0.2 warm-host writer, P1.5 CRUD, P1.6 gates, P1.7 reward). The decisions live as prose in `.plans/phase-1-implementation.md`, but until they are encoded as Rust types they cannot be referenced or unit-tested. The work here is therefore *type-first*: introduce the smallest possible set of new modules under `rmc-graph` that hold the canonical declarations of `WorkingSnapshot` (D1), `EditClass` + `AffectedSet` (D2), the invalidation matrix table (D3) and `Checkpoint` + `UndoLog` (D4). No mutation paths are wired up yet — that lands in M2a.

Alongside the contracts we ship a brand-new dev-only crate `rmc-spikes` containing two binaries: `ra_fanout` (Spike 1) and `cargo_latency` (Spike 2). Both produce JSON reports with hard go/no-go numbers — body-only re-extract < 500 ms and warm `cargo check` < ~2 s on the P0.4 pool. If either spike fails its threshold, the M2 plan must be revisited before P0.2 starts.

## New modules / files

- `crates/rmc-graph/src/working/mod.rs` — module root for D1 (`WorkingSnapshot`, session-id type, copy/publish ops).
- `crates/rmc-graph/src/working/snapshot.rs` — `WorkingSnapshot` + `init_from_published`, `working_dir_for`, `working_paths`, `publish_as_new_graph_id`. Owns the LMDB `mdb_copy` via `heed::Env::copy_to_path`.
- `crates/rmc-graph/src/working/identity.rs` — `SessionId(Uuid)`, `GraphId(String)`, `WorkingSnapshotIdentity { session_id, base_graph_id: GraphId, edit_seq }` (all newtype inner fields private; struct fields private with accessors).
- `crates/rmc-graph/src/affected/mod.rs` — module root for D2 + D3, re-exports `EditClass`, `Edit`, `AffectedSet`, `InvalidationRule`, `SubDb`, `InvalidationAction`, `invalidations_for`, `classify`, `expand`.
- `crates/rmc-graph/src/affected/edit.rs` — `Edit` enum (typed payload a CRUD op hands the engine) and `classify(&Edit) -> EditClass`. Classification is by construction — no diff inference.
- `crates/rmc-graph/src/affected/set.rs` — `AffectedSet`, `expand(class, edit, &ReverseDepGraph) -> AffectedSet`, and `ReverseDepGraph` (in-memory reverse adjacency of `crate_edges()`).
- `crates/rmc-graph/src/affected/matrix.rs` — `SubDb` enum (one variant per field of `GraphDatabases`), `InvalidationAction`, `InvalidationRule`, the static `INVALIDATION_MATRIX` table, and `invalidations_for(class) -> Vec<InvalidationRule>`. The function matches on `EditClass` alone (not on `(EditClass, SubDb)` pairs), so a missing rule for a sub-db is *not* a compile-time error; coverage is guaranteed by the documented runtime test `every_class_covers_every_sub_db` (see DD-1).
- `crates/rmc-graph/src/checkpoint/mod.rs` — module root for D4.
- `crates/rmc-graph/src/checkpoint/undo.rs` — `UndoEntry`, `UndoLog`, append-only on-disk format (`working/<session_id>/undo.log`), `record / restore / mark`.
- `crates/rmc-graph/src/checkpoint/checkpoint.rs` — `Checkpoint`, `JjOpId`, `UndoLogMarker`, `RaEditSeq`, plus `take_checkpoint`, `restore`.
- `crates/rmc-graph/src/lib.rs` — add `pub mod working;`, `pub mod affected;`, `pub mod checkpoint;`.
- `crates/rmc-graph/Cargo.toml` — add `uuid = { version = "1.10", features = ["v4", "serde"] }`.
- `crates/rmc-spikes/Cargo.toml` — new dev-only crate; `publish = false`; depends on `rmc-graph`, `rmc-indexing`, `anyhow`, `serde`, `serde_json`, `tracing`, `clap`.
- `crates/rmc-spikes/src/lib.rs` — shared helpers: `WorkspaceFixture`, `EditScenario`, `Report`, `measure_re_extract`.
- `crates/rmc-spikes/src/bin/ra_fanout.rs` — Spike 1.
- `crates/rmc-spikes/src/bin/cargo_latency.rs` — Spike 2.
- `Cargo.toml` (workspace) — add `"crates/rmc-spikes"` to `members`.

## Type definitions

### D1 — `crates/rmc-graph/src/working/identity.rs`

```rust
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(Uuid);

impl SessionId {
    /// Generate a fresh random session id.
    #[must_use]
    pub fn new() -> Self { Self(Uuid::new_v4()) }
    #[must_use]
    pub fn to_hex(self) -> String { self.0.simple().to_string() }
    #[must_use]
    pub fn as_uuid(self) -> Uuid { self.0 }
}

// A random id is the only sensible default, so `Default` matches `new()` and
// avoids the `new_without_default` lint.
impl Default for SessionId {
    fn default() -> Self { Self::new() }
}

/// Opaque published-graph identifier. Inner string is private so callers cannot
/// fabricate or mutate ids out of band.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GraphId(String);

impl GraphId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self { Self(id.into()) }
    #[must_use]
    pub fn as_str(&self) -> &str { &self.0 }
}

/// Identity tuple for a working snapshot. Decoupled from content fingerprint
/// by design — see D1. Two working snapshots with the same `(base_graph_id,
/// edit_seq)` but different `session_id` are distinct artifacts.
///
/// Fields are private: `edit_seq` is an invariant bumped only via
/// `bump_edit_seq` on the owning `WorkingSnapshot`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkingSnapshotIdentity {
    session_id: SessionId,
    base_graph_id: GraphId,
    edit_seq: u64,
}

impl WorkingSnapshotIdentity {
    #[must_use]
    pub fn session_id(&self) -> SessionId { self.session_id }
    #[must_use]
    pub fn base_graph_id(&self) -> &GraphId { &self.base_graph_id }
    #[must_use]
    pub fn edit_seq(&self) -> u64 { self.edit_seq }
}
```

### D1 — `crates/rmc-graph/src/working/snapshot.rs`

```rust
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use heed::{CompactionOption, Env, WithoutTls};
use thiserror::Error;

use crate::graph::storage::{GraphDatabases, GraphEnvOptions, GraphManifest, GraphPaths};
use super::identity::{GraphId, SessionId, WorkingSnapshotIdentity};

/// Failure modes of working-snapshot lifecycle operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WorkingSnapshotError {
    #[error("filesystem error preparing working dir")]
    Io(#[from] io::Error),
    #[error("LMDB copy/open failed")]
    Lmdb(#[source] heed::Error),
    #[error("manifest read/parse failed")]
    Manifest(#[source] crate::graph::storage::ManifestError),
    #[error("publish is not implemented in M0")]
    PublishUnimplemented,
}

pub struct WorkingSnapshot {
    identity: WorkingSnapshotIdentity,
    working_dir: PathBuf,
    env: Arc<Env<WithoutTls>>,
    dbs: GraphDatabases,
    base_manifest: GraphManifest,
}

impl WorkingSnapshot {
    /// Copy `<workspace_hash>/snapshots/<base_graph_id>/data.mdb` into a fresh
    /// working dir using `heed::Env::copy_to_path` (LMDB `mdb_copy`).
    ///
    /// # Errors
    /// Returns [`WorkingSnapshotError`] if the working dir cannot be created,
    /// the LMDB copy/open fails, or the base manifest cannot be read.
    pub fn init_from_published(
        paths: &GraphPaths,
        base_graph_id: &GraphId,
        env: GraphEnvOptions,
    ) -> Result<Self, WorkingSnapshotError> { todo!() }

    /// # Errors
    /// In M0 always returns [`WorkingSnapshotError::PublishUnimplemented`].
    pub fn publish_as_new_graph_id(&self) -> Result<GraphId, WorkingSnapshotError> { todo!() }

    /// # Errors
    /// Returns [`WorkingSnapshotError::Io`] if the working dir cannot be removed.
    pub fn drop_session(self) -> Result<(), WorkingSnapshotError> { todo!() }

    pub fn bump_edit_seq(&mut self) { self.identity.bump_edit_seq() }

    // Read-only accessors; `env`/`dbs` are intentionally not exposed mutably so
    // callers cannot corrupt edit-seq/undo invariants behind the snapshot.
    #[must_use]
    pub fn identity(&self) -> &WorkingSnapshotIdentity { &self.identity }
    #[must_use]
    pub fn working_dir(&self) -> &Path { &self.working_dir }
    #[must_use]
    pub fn env(&self) -> &Arc<Env<WithoutTls>> { &self.env }
    #[must_use]
    pub fn dbs(&self) -> &GraphDatabases { &self.dbs }
    #[must_use]
    pub fn base_manifest(&self) -> &GraphManifest { &self.base_manifest }
}

#[must_use]
pub fn working_dir(paths: &GraphPaths, session: SessionId) -> PathBuf { todo!() }
```

`WorkingSnapshotIdentity::bump_edit_seq` (a private/crate method on the identity)
increments `edit_seq` — the only mutation path for the field.

### D2 — `crates/rmc-graph/src/affected/edit.rs`

```rust
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use crate::graph::ids::NodeId;

/// Fully-qualified path of an item (e.g. `my_crate::module::Item`). Newtype so
/// the engine never passes a raw `String` where a qualified name is expected.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QualifiedName(String);

impl QualifiedName {
    #[must_use]
    pub fn new(path: impl Into<String>) -> Self { Self(path.into()) }
    #[must_use]
    pub fn as_str(&self) -> &str { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Edit {
    ModifyBody {
        target: NodeId,
        file: PathBuf,
        byte_span: (u32, u32),
        new_body: String,
    },
    ModifySignature {
        target: NodeId,
        file: PathBuf,
        new_signature_source: String,
    },
    ItemAddRemove {
        op: ItemMutation,
        parent_module: NodeId,
        target_qualified: QualifiedName,
    },
    ModuleTree {
        affected_files: Vec<PathBuf>,
        affected_crate: NodeId,
    },
    Macro { affected_crate: NodeId },
    CargoManifest { file: PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ItemMutation { Add, Remove }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EditClass {
    BodyOnly, SignatureOrVis, ItemAddRemove, ModuleTree, Macro, CargoManifest,
}

#[must_use]
pub fn classify(edit: &Edit) -> EditClass {
    match edit {
        Edit::ModifyBody { .. } => EditClass::BodyOnly,
        Edit::ModifySignature { .. } => EditClass::SignatureOrVis,
        Edit::ItemAddRemove { .. } => EditClass::ItemAddRemove,
        Edit::ModuleTree { .. } => EditClass::ModuleTree,
        Edit::Macro { .. } => EditClass::Macro,
        Edit::CargoManifest { .. } => EditClass::CargoManifest,
    }
}
```

### D2 — `crates/rmc-graph/src/affected/set.rs`

```rust
use thiserror::Error;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AffectedSet {
    pub dirty_files: BTreeSet<PathBuf>,
    pub dirty_crates: BTreeSet<NodeId>,
    pub reverse_dep_crates: BTreeSet<NodeId>,
    pub full_rebuild: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ReverseDepGraph {
    by_producer: BTreeMap<NodeId, BTreeSet<NodeId>>,
}

impl ReverseDepGraph {
    #[must_use]
    pub fn from_crate_edges<E: ToCrateEdge>(edges: impl IntoIterator<Item = E>) -> Self { todo!() }

    #[must_use]
    pub fn consumers_of(&self, producer: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.by_producer.get(&producer).into_iter().flat_map(|s| s.iter().copied())
    }
}

pub trait ToCrateEdge {
    fn consumer_crate(&self) -> NodeId;
    fn producer_crate(&self) -> NodeId;
}

/// Failure modes of affected-set expansion.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum InvalidationError {
    #[error("edit class {class:?} does not match edit variant {variant}")]
    ClassEditMismatch { class: EditClass, variant: &'static str },
    #[error("file {0} is not mapped to any crate")]
    FileNotInAnyCrate(PathBuf),
}

/// Expand an `(class, edit)` pair into the set of dirty files/crates.
///
/// # Errors
/// Returns [`InvalidationError::ClassEditMismatch`] if `class` and `edit`
/// disagree, or [`InvalidationError::FileNotInAnyCrate`] if `crate_of_file`
/// cannot map an affected file to a crate.
pub fn expand(
    class: EditClass,
    edit: &Edit,
    rdg: &ReverseDepGraph,
    crate_of_file: &dyn Fn(&std::path::Path) -> Option<NodeId>,
) -> Result<AffectedSet, InvalidationError> { todo!() }
```

### D3 — `crates/rmc-graph/src/affected/matrix.rs`

```rust
// `SubDb` has 15 variants — one per field of `GraphDatabases` (13 LMDB
// sub-databases plus the on-disk `Manifest` and the reserved
// `DescriptionsByTarget`; see Step 11 destructure and `ALL_SUB_DBS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SubDb {
    NodesById, BindingsById, BindingsByFromModule, BindingsByTarget,
    ChildrenByParent, UsagesById, UsagesByTarget, UsagesByConsumer,
    UsagesByConsumerFunction, SignaturesByTarget, StaticMetadataByTarget,
    EmbeddingsByTarget, DescriptionsByTarget, MetaByKey, Manifest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum InvalidationAction {
    Patch, ReDerive, ContentHashCache, Unchanged, Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InvalidationRule { pub table: SubDb, pub action: InvalidationAction }

/// Rules for `class`. Matches on `EditClass` only — a missing rule for some
/// sub-db is NOT a compile-time error (see DD-1); the runtime test
/// `every_class_covers_every_sub_db` guarantees coverage.
#[must_use]
pub fn invalidations_for(class: EditClass) -> Vec<InvalidationRule> {
    use EditClass::*; use InvalidationAction::*; use SubDb::*;
    let mut out = Vec::with_capacity(15);
    macro_rules! r { ($t:expr, $a:expr) => { out.push(InvalidationRule { table: $t, action: $a }); }; }
    match class {
        BodyOnly => {
            r!(NodesById, Unchanged);  r!(BindingsById, Unchanged);
            r!(BindingsByFromModule, Unchanged);  r!(BindingsByTarget, Unchanged);
            r!(ChildrenByParent, Unchanged);  r!(UsagesById, Patch);
            r!(UsagesByTarget, Patch);  r!(UsagesByConsumer, Patch);
            r!(UsagesByConsumerFunction, Patch);  r!(SignaturesByTarget, Unchanged);
            r!(StaticMetadataByTarget, ReDerive);  r!(EmbeddingsByTarget, ContentHashCache);
            r!(DescriptionsByTarget, ContentHashCache);  r!(MetaByKey, Patch);  r!(Manifest, Patch);
        }
        SignatureOrVis => {
            for t in [NodesById, BindingsById, BindingsByFromModule, BindingsByTarget] { r!(t, Patch); }
            r!(ChildrenByParent, Unchanged);
            for t in [UsagesById, UsagesByTarget, UsagesByConsumer, UsagesByConsumerFunction] { r!(t, Patch); }
            r!(SignaturesByTarget, ReDerive);  r!(StaticMetadataByTarget, ReDerive);
            r!(EmbeddingsByTarget, ContentHashCache);  r!(DescriptionsByTarget, ContentHashCache);
            r!(MetaByKey, Patch);  r!(Manifest, Patch);
        }
        ItemAddRemove => {
            for t in [NodesById, BindingsById, BindingsByFromModule, BindingsByTarget, ChildrenByParent,
                      UsagesById, UsagesByTarget, UsagesByConsumer, UsagesByConsumerFunction] { r!(t, Patch); }
            r!(SignaturesByTarget, ReDerive);  r!(StaticMetadataByTarget, ReDerive);
            r!(EmbeddingsByTarget, ContentHashCache);  r!(DescriptionsByTarget, ContentHashCache);
            r!(MetaByKey, Patch);  r!(Manifest, Patch);
        }
        ModuleTree => {
            for t in [NodesById, BindingsById, BindingsByFromModule, BindingsByTarget, ChildrenByParent,
                      UsagesById, UsagesByTarget, UsagesByConsumer, UsagesByConsumerFunction] { r!(t, Patch); }
            r!(SignaturesByTarget, Unchanged);  r!(StaticMetadataByTarget, Unchanged);
            r!(EmbeddingsByTarget, ContentHashCache);  r!(DescriptionsByTarget, ContentHashCache);
            r!(MetaByKey, Patch);  r!(Manifest, Patch);
        }
        Macro | CargoManifest => {
            for t in ALL_SUB_DBS {
                let a = match t {
                    EmbeddingsByTarget | DescriptionsByTarget => ContentHashCache,
                    _ => Full,
                };
                r!(*t, a);
            }
        }
    }
    out
}

pub(crate) const ALL_SUB_DBS: &[SubDb] = &[
    SubDb::NodesById, SubDb::BindingsById, SubDb::BindingsByFromModule,
    SubDb::BindingsByTarget, SubDb::ChildrenByParent, SubDb::UsagesById,
    SubDb::UsagesByTarget, SubDb::UsagesByConsumer, SubDb::UsagesByConsumerFunction,
    SubDb::SignaturesByTarget, SubDb::StaticMetadataByTarget, SubDb::EmbeddingsByTarget,
    SubDb::DescriptionsByTarget, SubDb::MetaByKey, SubDb::Manifest,
];
```

### D4 — `crates/rmc-graph/src/checkpoint/undo.rs`

```rust
use std::io;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct UndoEntry {
    pub sub_db: SubDb,
    pub key: Vec<u8>,
    pub prior_value: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoLogMarker { pub byte_offset: u64, pub entry_count: u64 }

/// Failure modes of the append-only undo log.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum UndoError {
    #[error("undo-log IO error")]
    Io(#[from] io::Error),
    #[error("undo-entry (de)serialization failed")]
    Codec(#[source] bincode::Error),
    #[error("LMDB write while restoring undo log")]
    Lmdb(#[source] heed::Error),
}

pub struct UndoLog {
    path: PathBuf,
    writer: BufWriter<File>,
    entry_count: u64,
}

impl UndoLog {
    /// Open (creating if absent) the append-only undo log under `working_dir`.
    ///
    /// # Errors
    /// Returns [`UndoError::Io`] if the file cannot be opened or its existing
    /// length/entry count cannot be read.
    pub fn open(working_dir: &Path) -> Result<Self, UndoError> { todo!() }

    /// # Errors
    /// Returns [`UndoError::Codec`]/[`UndoError::Io`] on serialize/write failure.
    pub fn record(&mut self, entry: UndoEntry) -> Result<(), UndoError> { todo!() }

    /// # Errors
    /// Returns [`UndoError::Io`] if the flush or position query fails.
    pub fn mark(&mut self) -> Result<UndoLogMarker, UndoError> { todo!() }

    /// Restore the log back to `marker`, replaying prior values into `dbs`.
    ///
    /// # Errors
    /// Returns [`UndoError`] if a flush, backward read/decode, LMDB write, or
    /// the final truncate fails.
    pub fn restore(&mut self, marker: UndoLogMarker, wtxn: &mut RwTxn<'_>,
                   dbs: &crate::graph::storage::GraphDatabases) -> Result<(), UndoError> { todo!() }

    #[must_use]
    pub fn entry_count(&self) -> u64 { self.entry_count }

    #[must_use]
    pub fn path(&self) -> &Path { &self.path }
}
```

### D4 — `crates/rmc-graph/src/checkpoint/checkpoint.rs`

```rust
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JjOpId(String);

impl JjOpId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self { Self(id.into()) }
    #[must_use]
    pub fn as_str(&self) -> &str { &self.0 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RaEditSeq(u64);

impl RaEditSeq {
    #[must_use]
    pub fn new(seq: u64) -> Self { Self(seq) }
    #[must_use]
    pub fn get(self) -> u64 { self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Checkpoint {
    pub jj_op_id: JjOpId,
    pub undo_log_marker: UndoLogMarker,
    pub ra_edit_seq: RaEditSeq,
    pub caches: (),  // content-hash keyed → self-heal
}

/// All-or-nothing restore failure. Caller MUST drop the working snapshot on Err.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RestoreError {
    #[error("undo-log restore failed")]
    Undo(#[from] UndoError),
    #[error("LMDB transaction failed during restore")]
    Lmdb(#[source] heed::Error),
    #[error("`jj op restore {op}` exited with status {status}")]
    JjRestore { op: String, status: i32 },
    #[error("failed to spawn `jj`")]
    JjSpawn(#[source] std::io::Error),
    #[error("rust-analyzer host rewind failed")]
    RaRewind(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Capture a checkpoint at the current undo-log + RA position.
///
/// # Errors
/// Returns [`UndoError`] if marking the undo log fails.
pub fn take_checkpoint(ws: &WorkingSnapshot, undo: &mut UndoLog,
                       jj_op_id: JjOpId, ra_edit_seq: RaEditSeq) -> Result<Checkpoint, UndoError> { todo!() }

/// All-or-nothing restore. Caller MUST drop the working snapshot on Err.
///
/// # Errors
/// Returns [`RestoreError`] if the LMDB undo replay, `jj op restore`, or the
/// RA host rewind fails; on any error the working snapshot is left unusable and
/// must be dropped.
pub fn restore(ws: &WorkingSnapshot, checkpoint: &Checkpoint,
               undo: &mut UndoLog, ra_host: &mut dyn RaHostHandle) -> Result<(), RestoreError> { todo!() }

pub trait RaHostHandle {
    fn current_edit_seq(&self) -> RaEditSeq;

    /// # Errors
    /// Returns a boxed error if the host cannot rewind to `target`.
    fn rewind_to(&mut self, target: RaEditSeq) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}
```

## Step-by-step implementation

### D1 — Working snapshot

1. **Add module skeleton.** Create `working/{mod.rs, identity.rs, snapshot.rs}`; add `pub mod working;` to `lib.rs`. `todo!()` bodies. **VERIFY**: `cargo check -p rmc-graph`.
2. **Implement `working_dir`.** `paths.root_dir.join("working").join(session.to_hex())`. **VERIFY**: `working_dir_is_under_workspace_hash`.
3. **Implement `init_from_published`.** `fs::create_dir_all(working_dir)`; open source env read-only at `paths.snapshot_dir(base_graph_id.as_str())`; `source_env.copy_to_path(working_dir.join("data.mdb"), CompactionOption::Disabled)`; open destination env; open `GraphDatabases::open`. Read `manifest.json` via `storage::read_manifest`; clone as `base_manifest`. Map `io::Error`/`heed::Error`/`ManifestError` into `WorkingSnapshotError`. Construct identity with fresh `SessionId::new()` and `edit_seq: 0`. **VERIFY**: `init_copies_full_state`.
4. **Implement `drop_session`.** Drop `Arc<Env>`, `fs::remove_dir_all` ignoring `NotFound`. **VERIFY**: `init_then_drop_leaves_no_residue`.
5. **Implement `publish_as_new_graph_id` (skeleton).** For M0 return `Err(WorkingSnapshotError::PublishUnimplemented)`. Signature compiles. **VERIFY**: `#[ignore]` test stub.

### D2 — Classifier + expander

6. **Add `affected` module skeleton.** **VERIFY**: `cargo check`.
7. **Implement `classify`.** Trivial `match`. **VERIFY**: `classifier_maps_every_variant`.
8. **Implement `ReverseDepGraph::from_crate_edges`.** Iterate, insert `(producer, consumer)`. **VERIFY**: synthetic A→B, A→C, B→C round-trips `consumers_of`.
9. **Implement `expand`.** Per class:
   - `BodyOnly` → `dirty_files = {edit.file}`, `dirty_crates = {crate_of_file(file)}`, no reverse-deps.
   - `SignatureOrVis`, `ItemAddRemove` → plus transitive reverse-dep closure via BFS.
   - `ModuleTree` → similar, with `affected_files` from `Edit::ModuleTree`.
   - `Macro` → dirty crate plus transitive consumers.
   - `CargoManifest` → `full_rebuild = true`.
   A `crate_of_file` miss returns `InvalidationError::FileNotInAnyCrate`; a
   class/variant mismatch returns `InvalidationError::ClassEditMismatch`.
   **VERIFY**: per-class table-driven test.
10. **Document `crate_of_file` injection.** Closure provided by working snapshot layer using `file → crate_id` map derived from `Node` records.

### D3 — Matrix in code

11. **Add `SubDb` enum + storage-layout guard.** This destructure is a genuine
    compile-time guard that every `GraphDatabases` field has a corresponding
    `SubDb` variant (it breaks to compile if a field is added/removed). It does
    NOT make the D3 *matrix* exhaustive — that is the runtime test
    `every_class_covers_every_sub_db` (see DD-1). The destructure must list all
    15 fields, including `manifest` and `descriptions_by_target`:
    ```rust
    #[cfg(test)]
    fn _matches_storage_layout() {
        let _: fn(crate::graph::storage::GraphDatabases) = |dbs| {
            let crate::graph::storage::GraphDatabases {
                meta_by_key: _, nodes_by_id: _, bindings_by_id: _,
                bindings_by_from_module: _, bindings_by_target: _,
                children_by_parent: _, usages_by_id: _, usages_by_target: _,
                usages_by_consumer: _, usages_by_consumer_function: _,
                signatures_by_target: _, static_metadata_by_target: _,
                embeddings_by_target: _, descriptions_by_target: _, manifest: _,
            } = dbs;
        };
    }
    ```
    (15 fields ↔ 15 `SubDb` variants ↔ 15 entries in `ALL_SUB_DBS`.)
12. **Implement `invalidations_for`.** Write the `match` per the prose D3 table.
13. **Cite source.** Top-of-file doc-comment links to `.plans/phase-1-implementation.md`.

### D4 — Checkpoint + undo log

14. **Add `checkpoint` module skeleton.** **VERIFY**: `cargo check`.
15. **Implement `UndoLog::open`.** `OpenOptions::read+write+create+append`; track existing length + count. **VERIFY**: `open_then_reopen_preserves_count`.
16. **Implement `record`.** bincode-serialize entry, `u32` LE length prefix + bytes, increment count. Flush on `mark` and `drop`. **VERIFY**: round-trip.
17. **Implement `mark`.** Flush, return `{ byte_offset: file.stream_position(), entry_count }`. **VERIFY**: trivial.
18. **Implement `restore`.** Flush; seek to EOF; walk backwards reading length prefixes; per entry dispatch on `sub_db` (primary tables → put/delete; DUP_SORT → use `delete_one_duplicate` or `put`; `MetaByKey` → str-byte key; `Manifest` → bincoded `GraphManifest`, rewrite `manifest.json` outside txn). Truncate log to `marker.byte_offset`; reset `entry_count = marker.entry_count`. All `seek`/`set_len`/`stream_position` and bincode/heed calls map their error into `UndoError` via `?` — none of them `unwrap()`/`expect()`. Add `# Errors` to the doc; this fn does not panic (`# Panics`: none). **VERIFY**: `record_then_restore_to_zero_recovers_pre_state`.
19. **Implement `take_checkpoint`.** Returns `Checkpoint { jj_op_id, undo_log_marker: undo.mark()?, ra_edit_seq, caches: () }`. `# Errors`: propagates `UndoError` from `mark`.
20. **Implement `restore` orchestrator.** Open `wtxn`; `undo.restore` (map `heed::Error` → `RestoreError::Lmdb`); commit. Then `Command::new("jj").args(["op","restore", checkpoint.jj_op_id.as_str()]).status()` (gated `#[cfg(feature = "jj")]` for M0): map spawn failure to `RestoreError::JjSpawn`, and a non-zero/`None` exit code to `RestoreError::JjRestore { op, status }` — do not `unwrap()` the `ExitStatus`. Then `ra_host.rewind_to`, wrapping its boxed error in `RestoreError::RaRewind`. Any Err → caller drops working snapshot. `# Errors`: see [`RestoreError`]; `# Panics`: none. **VERIFY**: `restore_round_trips_5_patches`.
21. **Wire `bump_edit_seq` on `WorkingSnapshot`** for the patcher (P0.2 calls it).

### Spike 1 — RA fan-out

22. **Create `rmc-spikes` crate.** `publish = false`, two `[[bin]]` entries. Add to workspace `members`. **VERIFY**: `cargo check -p rmc-spikes`.
23. **`WorkspaceFixture` helper.** `from_env()` reads `RMC_SPIKE_WORKSPACE` (default = rmc workspace itself for smoke).
24. **`EditScenario` enum.** Mirror D2 classes plus textual patches:
    - `BodyOnly`: replace leaf-fn body with `return Default::default();`.
    - `SignatureOrVis`: `pub fn` → `pub(crate) fn`.
    - `ItemAdd`: append `pub fn __spike_added() {}`.
    - `ModuleTree`: insert `pub mod __spike_module;` + a 1-file module.
    - `Macro`: change a `macro_rules!` body.
    - `CargoManifest`: bump a patch version.
    Record `(file, original_bytes)` so spike can revert.
25. **`ra_fanout.rs` binary.**
    ```rust
    fn main() -> Result<()> {
        let fx = WorkspaceFixture::from_env()?;
        let mut report = Report::default();
        report.loc = fx.loc();
        let t0 = Instant::now();
        let mut loaded = rmc_graph::graph::loader::load(&fx.root)?;
        report.cold_load_ms = t0.elapsed().as_millis() as u64;
        let t1 = Instant::now();
        let _model = rmc_graph::graph::extract::extract(&loaded);
        report.cold_extract_ms = t1.elapsed().as_millis() as u64;
        for scenario in EditScenario::menu() {
            scenario.apply_to_disk()?;
            let t = Instant::now();
            loaded.vfs.set_file_contents(
                scenario.vfs_id(&loaded.vfs)?, Some(scenario.new_bytes()),
            );
            let _ = rmc_graph::graph::extract::extract(&loaded);
            report.per_class.insert(scenario.class(), t.elapsed().as_millis() as u64);
            scenario.revert_on_disk()?;
        }
        println!("{}", serde_json::to_string_pretty(&report)?);
        assert_pass(&report);  // body-only < 500ms
        Ok(())
    }
    ```
    Pass: `report.per_class[BodyOnly] < 500`. Fail exits with code 2.
26. **Document Spike 1's caveats.** This measures whole-workspace re-extract (upper bound). If it passes, the optimised P0.2 path is guaranteed to pass.

### Spike 2 — Cargo gate latency

27. **`cargo_latency.rs`.**
    ```rust
    fn main() -> Result<()> {
        let pool_dir = std::env::var("RMC_SPIKE_POOL")?;
        let mut report = PoolReport::default();
        for crate_dir in list_crates(&pool_dir)? {
            run_cargo(&crate_dir, &["check", "--message-format=json"])?;  // warm cache
            let scoped_test = pick_one_test_target(&crate_dir)?;
            let mut warm = Vec::new();
            for _ in 0..3 {
                let t = Instant::now();
                run_cargo(&crate_dir, &["check"])?;
                warm.push(t.elapsed().as_millis());
            }
            let mut test_warm = Vec::new();
            for _ in 0..3 {
                let t = Instant::now();
                run_cargo(&crate_dir, &["test", "--no-fail-fast", "--", "--quiet", &scoped_test])?;
                test_warm.push(t.elapsed().as_millis());
            }
            report.crates.push(CrateReport {
                name: crate_name(&crate_dir),
                warm_check_p50_ms: median(&warm),
                warm_test_p50_ms:  median(&test_warm),
            });
        }
        println!("{}", serde_json::to_string_pretty(&report)?);
        assert!(report.crates.iter().all(|c| c.warm_check_p50_ms < 2000));
        Ok(())
    }
    ```
28. **Document Spike 2's scoping.** `cargo check` (cheap) and `cargo test` filtered to one test. If median > 2s, design switches to RA-based type-check (P1.7).

### Wrap-up

29. **M0 status section** for human paste.
30. **Gate sign-off:** both spike binaries exit zero → M0 green. Else revise M2 plan.

## Tests

### D1
- **`init_copies_full_state`** — build tiny published snapshot via `persist_test_model`; `init_from_published`; walk both envs and assert `nodes_by_id` keys identical.
- **`init_then_drop_leaves_no_residue`** — after `drop_session`, working dir gone, `snapshot_dir(base_graph_id)` untouched.
- **`identity_edit_seq_starts_at_zero`** — invariant; bumped only by `bump_edit_seq`.

### D2 classifier
- **`classifier_maps_every_variant`** — one row per `Edit` variant.
- **`body_only_edit_classifies`** — `Edit::ModifyBody { ... }` → `EditClass::BodyOnly`.
- **`cargo_toml_edit_classifies_as_full_rebuild_class`** — `Edit::CargoManifest { ... }` → `EditClass::CargoManifest` → `expand` sets `full_rebuild = true`.

### D2 expander
- **`expand_body_only_has_empty_reverse_deps`** — A→B→C, edit fn in C, `reverse_dep_crates` empty.
- **`expand_sig_or_vis_propagates_to_reverse_deps`** — edit `pub fn` in A, transitive closure ⊇ {B, C}.
- **`expand_macro_includes_all_consumers`** — macro edit in A; reverse-dep closure.
- **`expand_cargo_sets_full_rebuild`**.

### D3 matrix
- **`every_class_covers_every_sub_db`** — the DD-1 coverage guarantee: iterate `EditClass::ALL` (6) × `SubDb::ALL` (15), assert `invalidations_for(class)` contains exactly one rule per sub-db. This is the *runtime* substitute for the (impossible) compile-time exhaustiveness, since `invalidations_for` matches on `EditClass` alone. Requires `EditClass::ALL: [EditClass; 6]` and `SubDb::ALL` (aliasing `ALL_SUB_DBS`, 15 entries).
- **`body_only_does_not_touch_nodes`** — `BodyOnly` contains `Rule { table: NodesById, action: Unchanged }`.
- **`cargo_is_full_rebuild_except_caches`** — `CargoManifest` is `Full` on every sub-db except the content-hash caches.
- **`matrix_matches_design_doc_prose`** — golden table assertion mirroring the doc grid.
- **`_matches_storage_layout`** — storage-layout compile-time guard from Step 11 (guards `SubDb` ↔ `GraphDatabases` field parity, NOT matrix coverage).

### D4
- **`open_then_reopen_preserves_count`**.
- **`record_then_restore_to_zero_recovers_pre_state`** — synthetic env, 5 mutations recorded, restore to pre-marker → byte-equal.
- **`restore_handles_dup_sort`** — `BindingsByTarget` (DUP_SORT) record/delete/restore.
- **`restore_truncates_log_file`** — post-restore file size == marker.
- **`take_then_restore_round_trips`** — 5 patches via stub patcher → restore → byte-equal.
- **`restore_failure_signals_drop_required`** — mock `RaHostHandle::rewind_to` returns `Err`.

### Spikes
- **`ra_fanout_runs_on_rmc_workspace`** (`#[ignore]`) — exits 0.
- **`cargo_latency_runs_on_one_crate`** (`#[ignore]`) — exits 0.

## Open decisions / risks

- **Source of reverse-dep graph.** `OpenedSnapshot::crate_edges()` already exists. M0 ships only `ToCrateEdge` trait + adapter; **the `ReverseDepGraph::from_opened_snapshot` constructor lands in P0.2**.
- **Macro-vs-ItemAdd detection.** No `diff_to_edit_class` function — caller picks the `Edit` variant. Construction-time choice.
- **LMDB copy cost.** With `DEFAULT_MAP_SIZE = 1 GiB`, worst-case `mdb_copy` ≈ on-disk size. Rmc workspace ~50 MB → < 200 ms on SSD. 50 steps/episode → < 5 ms amortized per step.
- **Undo log size growth.** Per `UndoEntry`: ~300B × ~10 entries for body-only = a few KB; ModuleTree may reach hundreds of KB. 50 steps × ~1 MB worst-case = 50 MB; acceptable.
- **`Manifest` table is special.** Lives on disk, not LMDB. Restore rewrites `manifest.json` after committing LMDB txn — atomicity vs LMDB is weaker. Mitigation: patcher recomputes counts from `meta_by_key` on next open.
- **`SubDb`/`GraphDatabases` field parity (DD-1).** `SubDb` has 15 variants, so the Step 11 `_matches_storage_layout` destructure must list all 15 `GraphDatabases` fields — including `manifest` and `descriptions_by_target` (the two previously omitted). These two fields must therefore exist on `GraphDatabases` (even if `descriptions_by_target` is an empty/placeholder sub-DB until P1.2) for the parity guard to compile. The D3 matrix row for `DescriptionsByTarget` is already correct; P1.2 populates the sub-DB.
- **`StaticMetadataByTarget` for `BodyOnly`.** Rule is `ReDerive`; patcher elides via precondition check.
- **Spike 1 over-estimates** — passes guarantee P0.2 will too.
- **Spike 2 depends on P0.4 pool** — runs against rmc alone if pool absent (smoke).
- **No `jj` crate dep in M0.** Restore shells out to `jj op restore`.
- **Determinism.** Both affected-set and matrix are pure functions; `ReverseDepGraph::from_crate_edges` iterates BTreeMap order.


---

