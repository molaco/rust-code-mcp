# Section C — P0.2 Warm-Host Incremental Writer + P0.3 jj Rollback

## Overview

This slice is the **critical path** for the whole project: P0.2 (warm host + incremental writer) feeds P1.5 (CRUD) feeds P1.7 (reward) feeds P1.8 (episode runner). Every other phase can be built against the slow cold-rebuild path, but the episode loop cannot run faster than what the warm host delivers. The lethal item is `sub-500ms re-extract + LMDB patch on a body-only edit in a 100k-LOC workspace`; if that fails, RL is infeasible.

The architectural shift is from "build snapshot → discard `RootDatabase` → query LMDB" to "open snapshot once → keep `RootDatabase` warm in a `WorkspaceHost` → edits go through `set_file_text` → re-extract just the affected crates → diff against existing content-addressed LMDB keys → patch deltas under one write txn → log inverses to an undo log". The current query layer (`OpenedSnapshot`, `query/*`) is unchanged. The **working snapshot** (D1) is an `mdb_copy` of the published base, opened with `WithoutTls + write_txn` enabled, and lives under `working/<session_id>/`.

P0.3 wraps this in a **Checkpoint contract** spanning source (jj op id), graph (undo-log marker), and RA host (edit-seq). `rollback()` runs `jj op restore <op_id>`, replays inverse `set_file_text` calls on the warm RA database to mark, then replays the LMDB undo log in reverse. The fallback for divergence: drop the working snapshot, copy the base again, re-open the warm host (slow path; tracked but never the hot path).

## New modules / files

- `crates/rmc-graph/src/host/mod.rs` — `WorkspaceHost` + lifecycle. Exports `WorkspaceHost`, `FileEdit`, `EditSeq`, `EditClass`, `Checkpoint`, `HostError`.
- `crates/rmc-graph/src/host/edits.rs` — `FileEdit`, `EditClass`, `EditSeq`, `apply_edits`.
- `crates/rmc-graph/src/host/file_to_crate.rs` — bidirectional cache `PathBuf → SmallVec<NodeId>`, built once at host open by walking `Node.kind == Item` grouped by `Node.file → Node.crate_id`.
- `crates/rmc-graph/src/host/affected.rs` — D2 algorithm using `crate_edges` reversed.
- `crates/rmc-graph/src/host/re_extract.rs` — per-crate emit. Refactors `extract::extract` so the same helpers can be driven on a subset of `local_crates`. Output: `PartialExtractionModel`.
- `crates/rmc-graph/src/host/diff_patch.rs` — the LMDB diff-patch. Owns per-sub-DB delete/insert logic, DUP_SORT key/value pair semantics, `meta_by_key` counter updates, manifest counter write-back.
- `crates/rmc-graph/src/host/undo_log.rs` — `UndoLog`, `UndoOp` (one variant per primary sub-DB + each DUP_SORT secondary), `UndoMarker(EditSeq)` (private inner).
- `crates/rmc-graph/src/host/rollback.rs` — `Checkpoint::take`, `Checkpoint::restore`, jj wrapper via `tokio::process::Command`.
- `crates/rmc-graph/src/host/working_snapshot.rs` — D1 machinery: `WorkingSnapshot::init_from_base` using heed `env.copy_to_path`.
- `crates/rmc-graph/benches/incremental_extract.rs` — criterion bench for the 5 edit classes.
- `crates/rmc-graph/src/host/tests/` — differential tests.

Refactored existing files:
- `crates/rmc-graph/src/graph/extract.rs`: split `extract` into `extract_full(loaded)` (current shape) and `extract_partial(loaded, crates)`. The per-crate `emit_crate` stays; callers move into `extract_partial`. `extract_bindings`, `extract_impl_items`, `extract_attributes`, `extract_signatures`, `extract_statics`, `extract_usages` take a `local_crates: &[Crate]` arg.
- `crates/rmc-graph/src/graph/snapshot.rs`: `write_model` factors a helper `apply_full_model(env, dbs, model)`. `binding_id_for` / `usage_id_for` become `pub(in crate::graph)`.
- `crates/rmc-graph/src/graph/mod.rs`: `pub mod host;`.

## Type definitions

```rust
// crates/rmc-graph/src/host/mod.rs

pub struct WorkspaceHost {
    analysis: AnalysisHost,          // ra_ap_ide::AnalysisHost; warm RootDatabase
    vfs: Vfs,
    workspace_root: PathBuf,
    local_crates: Vec<Crate>,
    working: WorkingSnapshot,
    env: Arc<Env<WithoutTls>>,
    dbs: GraphDatabases,
    edit_seq: EditSeq,
    undo: UndoLog,
    file_to_crate: HashMap<PathBuf, SmallVec<[NodeId; 2]>>,
    crate_id_to_handle: HashMap<NodeId, Crate>,
    // Prior text of every file edited since the last `Checkpoint::take`, drained
    // by `take`. Used to replay inverse `set_file_text` on rollback (Step 3/10/11).
    recent_file_prior_text: HashMap<PathBuf, String>,
    // Cargo target-kind caches, populated once at host open (Step 5). Used by the
    // partial extractor; ModuleTree edits do NOT invalidate them (see Open decisions).
    crate_target_kinds_by_name: HashMap<String, TargetKind>,
    crate_target_kinds_by_root_file: HashMap<PathBuf, TargetKind>,
    // Memoised reverse-dep index for D2 affected-set expansion (Step 4).
    reverse_dep_index: HashMap<NodeId, Vec<NodeId>>,
    locks: Locks,
}

#[derive(Clone)]
pub struct Locks {
    workspace_locks: Arc<dyn WorkspaceLockHandle>,
}

impl Locks {
    #[must_use]
    pub fn new(workspace_locks: Arc<dyn WorkspaceLockHandle>) -> Self {
        Self { workspace_locks }
    }
}

impl WorkspaceHost {
    /// Crates the path resolves to when `file_to_crate` has no entry (e.g. a
    /// freshly created file not yet in the index): falls back to the crate whose
    /// root directory is the longest prefix of `path`. Returns empty if none.
    fn fallback_crates_for_path(&self, path: &Path) -> SmallVec<[NodeId; 2]> { /* … */ }
}
```

`HostError` is the single `thiserror` enum for every fallible host operation
(§9 decision rule: callers branch on failure mode — restore-on-error vs
host-reject vs cold-rebuild). It preserves source errors via `#[source]`/`#[from]`:

```rust
// crates/rmc-graph/src/host/mod.rs

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HostError {
    /// The edited path is not tracked by the warm host's VFS.
    #[error("unknown file: {0}")]
    UnknownFile(PathBuf),
    /// The edit class requires a full cold rebuild (CargoManifest, proc-macro).
    #[error("edit requires a cold rebuild")]
    ColdRebuildRequired,
    /// A persisted LMDB key/value or meta counter was not the expected width or
    /// shape. Corruption — NOT a local invariant.
    #[error("corrupt persisted record in {sub_db}: {detail}")]
    CorruptRecord { sub_db: &'static str, detail: String },
    /// A primary record expected to exist (e.g. when computing a remove's
    /// inverse) was missing under the write txn.
    #[error("expected record missing in {sub_db}")]
    MissingRecord { sub_db: &'static str },
    /// The undo log was empty or its top batch did not match the marker when a
    /// pop was expected during rollback.
    #[error("undo log inconsistent during rollback")]
    UndoLogInconsistent,
    /// `jj` subprocess failed (non-zero exit or spawn error).
    #[error("jj command failed: {0}")]
    Jj(String),
    /// Post-rollback state diverged from the checkpoint; the working snapshot was
    /// reopened from base (slow path). Surfaced so callers can react.
    #[error("rollback diverged from checkpoint; reopened from base")]
    RollbackDiverged,
    /// Underlying LMDB / heed failure.
    #[error("storage error")]
    Storage(#[source] heed::Error),
    /// Underlying filesystem / IO failure.
    #[error("io error")]
    Io(#[source] std::io::Error),
    /// Bincode (de)serialization failure on a persisted record.
    #[error("serialization error")]
    Serialize(#[source] bincode::Error),
}

impl From<heed::Error> for HostError {
    fn from(e: heed::Error) -> Self { Self::Storage(e) }
}
impl From<std::io::Error> for HostError {
    fn from(e: std::io::Error) -> Self { Self::Io(e) }
}
impl From<bincode::Error> for HostError {
    fn from(e: bincode::Error) -> Self { Self::Serialize(e) }
}
```

```rust
// crates/rmc-graph/src/host/edits.rs

#[derive(Debug, Clone)]
pub struct FileEdit {
    pub path: PathBuf,           // workspace-relative
    pub new_text: String,
    pub edit_class: EditClass,   // set by CRUD layer, NOT inferred
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EditClass {
    BodyOnly, SignatureOrVis, ItemAddRemove, ModuleTree, Macro, CargoManifest,
}

/// Monotonic edit sequence number. Inner value is private so callers cannot
/// fabricate out-of-order markers; construct via `apply_edits` / accessors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EditSeq(u64);

impl EditSeq {
    #[must_use]
    pub(crate) fn next(self) -> Self { Self(self.0 + 1) }
    #[must_use]
    pub fn get(self) -> u64 { self.0 }
}

impl WorkspaceHost {
    /// Apply a batch of file edits to the warm host and patch the working LMDB.
    ///
    /// # Errors
    /// Returns [`HostError::UnknownFile`] if an edited path is not tracked by the
    /// VFS, [`HostError::ColdRebuildRequired`] for `CargoManifest`/macro classes,
    /// and [`HostError::Storage`]/[`HostError::Serialize`]/[`HostError::CorruptRecord`]
    /// on persistence failures.
    pub fn apply_edits(&mut self, edits: &[FileEdit]) -> Result<EditSeq, HostError>;
}
```

```rust
// crates/rmc-graph/src/host/re_extract.rs

#[derive(Debug, Clone)]
pub struct PartialExtractionModel {
    dirty_crates: Vec<NodeId>,
    nodes: BTreeMap<NodeId, Node>,
    bindings: Vec<Binding>,
    usages: Vec<Usage>,
    contains: Vec<(NodeId, NodeId)>,
    signatures: Vec<(NodeId, FunctionSignature)>,
    statics: Vec<(NodeId, StaticMetadata)>,
}

impl PartialExtractionModel {
    #[must_use]
    pub fn dirty_crates(&self) -> &[NodeId] { &self.dirty_crates }
    #[must_use]
    pub fn nodes(&self) -> &BTreeMap<NodeId, Node> { &self.nodes }
    // …remaining read-only accessors mirror the fields.
}

pub(crate) fn extract_partial(loaded: &LoadedWorkspace, crates: &[Crate]) -> PartialExtractionModel;
```

```rust
// crates/rmc-graph/src/host/diff_patch.rs

#[derive(Debug, Default)]
#[non_exhaustive]
pub struct DiffPatch {
    node_inserts: Vec<Node>,
    node_updates: Vec<Node>,          // same key, different bincode
    node_removes: Vec<NodeId>,
    binding_inserts: Vec<(BindingId, Binding)>,
    binding_removes: Vec<BindingId>,
    usage_inserts: Vec<(UsageId, Usage)>,
    usage_removes: Vec<UsageId>,
    contains_inserts: Vec<(NodeId, NodeId)>,
    contains_removes: Vec<(NodeId, NodeId)>,
    signature_inserts: Vec<(NodeId, FunctionSignature)>,
    signature_removes: Vec<NodeId>,
    static_inserts: Vec<(NodeId, StaticMetadata)>,
    static_removes: Vec<NodeId>,
}

impl DiffPatch {
    #[must_use]
    pub fn is_empty(&self) -> bool { /* all vecs empty */ }
    #[must_use]
    pub(crate) fn size_hint(&self) -> usize { /* sum of vec lens */ }
    // …read-only accessors per field as needed by tests.
}

impl WorkspaceHost {
    /// Diff a re-extracted partial model against the existing dirty-crate records.
    ///
    /// # Errors
    /// [`HostError::Storage`] on read-txn failure, [`HostError::Serialize`] on a
    /// bincode comparison, [`HostError::CorruptRecord`] if a persisted key is not
    /// the expected width.
    pub(crate) fn compute_patch(
        &self,
        partial: &PartialExtractionModel,
    ) -> Result<DiffPatch, HostError>;

    /// Apply a computed patch under one write txn, recording inverses to the undo log.
    ///
    /// # Errors
    /// [`HostError::Storage`] on the write txn, [`HostError::MissingRecord`] if a
    /// record slated for removal is absent, [`HostError::CorruptRecord`] on a
    /// malformed meta counter.
    ///
    /// # Cancellation
    /// Not cancellation-safe across the write txn: the synchronous critical
    /// section runs to completion within one `env.write_txn()` and either commits
    /// atomically or the txn is dropped (rolled back). Callers must not interleave
    /// other host mutations between `compute_patch` and `apply_patch`.
    pub(crate) fn apply_patch(
        &mut self,
        patch: DiffPatch,
        next_seq: EditSeq,
    ) -> Result<(), HostError>;
}
```

```rust
// crates/rmc-graph/src/host/undo_log.rs

/// Marks a point in the undo log to roll back to. Inner `EditSeq` is private so
/// callers cannot construct a marker that does not correspond to a real batch.
#[derive(Debug, Clone, Copy)] pub struct UndoMarker(EditSeq);

impl UndoMarker {
    #[must_use]
    pub(crate) fn seq(self) -> EditSeq { self.0 }
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum UndoOp {
    NodeUpsert { key: [u8;32], prior: Option<Node> },
    NodeRemove { key: [u8;32], prior: Node },
    BindingUpsert { key: [u8;32], prior: Option<Binding> },
    BindingRemove { key: [u8;32], prior: Binding },
    UsageUpsert { key: [u8;32], prior: Option<Usage> },
    UsageRemove { key: [u8;32], prior: Usage },
    SignatureUpsert { key: [u8;32], prior: Option<FunctionSignature> },
    SignatureRemove { key: [u8;32], prior: FunctionSignature },
    StaticUpsert { key: [u8;32], prior: Option<StaticMetadata> },
    StaticRemove { key: [u8;32], prior: StaticMetadata },
    BindingByFromModuleInsert { key: [u8;32], value: [u8;32] },
    BindingByFromModuleDelete { key: [u8;32], value: [u8;32] },
    BindingByTargetInsert { key: [u8;32], value: [u8;32] },
    BindingByTargetDelete { key: [u8;32], value: [u8;32] },
    ChildrenByParentInsert { key: [u8;32], value: [u8;32] },
    ChildrenByParentDelete { key: [u8;32], value: [u8;32] },
    UsagesByTargetInsert { key: [u8;32], value: [u8;32] },
    UsagesByTargetDelete { key: [u8;32], value: [u8;32] },
    UsagesByConsumerInsert { key: [u8;32], value: [u8;32] },
    UsagesByConsumerDelete { key: [u8;32], value: [u8;32] },
    UsagesByConsumerFunctionInsert { key: [u8;32], value: [u8;32] },
    UsagesByConsumerFunctionDelete { key: [u8;32], value: [u8;32] },
    MetaCounter { name: &'static str, prior_le_bytes: [u8;8] },
}

#[derive(Debug, Default)] pub struct UndoBatch { seq: EditSeq, ops: Vec<UndoOp> }
#[derive(Debug, Default)] pub struct UndoLog { batches: Vec<UndoBatch> }

impl UndoBatch {
    #[must_use]
    pub(crate) fn new(seq: EditSeq, cap: usize) -> Self {
        Self { seq, ops: Vec::with_capacity(cap) }
    }
    #[must_use]
    pub(crate) fn seq(&self) -> EditSeq { self.seq }
    pub(crate) fn push(&mut self, op: UndoOp) { self.ops.push(op); }
    pub(crate) fn ops(&self) -> &[UndoOp] { &self.ops }
}

impl UndoLog {
    pub(crate) fn push(&mut self, batch: UndoBatch) { self.batches.push(batch); }
    #[must_use]
    pub(crate) fn last(&self) -> Option<&UndoBatch> { self.batches.last() }
    /// Pop the top batch.
    ///
    /// # Errors
    /// [`HostError::UndoLogInconsistent`] if the log is empty (the caller checked
    /// `last()` immediately before, so emptiness means a concurrency/logic bug in
    /// the rollback loop — surfaced rather than panicked, since the log is shared
    /// mutable state, not a local invariant).
    pub(crate) fn pop(&mut self) -> Result<UndoBatch, HostError> {
        self.batches.pop().ok_or(HostError::UndoLogInconsistent)
    }
}
```

```rust
// crates/rmc-graph/src/host/rollback.rs

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Checkpoint {
    jj_op_id: JjOpId,                         // DD-4: opaque jj op-log handle, not stringly-typed (§7)
    file_prior_text: HashMap<PathBuf, String>,
    edit_seq_marker: EditSeq,
}

impl Checkpoint {
    /// Capture a checkpoint across source (jj op id), graph (edit-seq marker), and
    /// the drained prior-text map for inverse `set_file_text` replay.
    ///
    /// # Errors
    /// [`HostError::Jj`] if `jj op log` fails to report the current op id. Fallible
    /// because it shells out to `jj` (the op log is external state, not a local
    /// invariant); hence `Result<Checkpoint, HostError>`, not `-> Self`.
    pub async fn take(host: &mut WorkspaceHost) -> Result<Checkpoint, HostError>;

    #[must_use]
    pub fn jj_op_id(&self) -> &str { self.jj_op_id.as_str() }
    #[must_use]
    pub fn edit_seq_marker(&self) -> EditSeq { self.edit_seq_marker }
    #[must_use]
    pub(crate) fn file_prior_text(&self) -> &HashMap<PathBuf, String> { &self.file_prior_text }
}

impl WorkspaceHost {
    /// Roll back source, RA host, and LMDB to the given checkpoint.
    ///
    /// # Errors
    /// [`HostError::Jj`] if `jj op restore` fails, [`HostError::Storage`] on the
    /// LMDB write txn, [`HostError::UndoLogInconsistent`] on undo-log mismatch, and
    /// [`HostError::RollbackDiverged`] when the post-rollback state diverged and the
    /// working snapshot was reopened from base (slow path).
    pub async fn rollback(&mut self, ck: &Checkpoint) -> Result<(), HostError>;
}
```

## Step-by-step implementation

### Step 1 — Refactor `extract.rs` to expose per-crate emission

WHERE: `crates/rmc-graph/src/graph/extract.rs`. DEPENDS: nothing.

(a) Split into `extract_full` (calls `extract_partial(loaded, &loaded.local_crates)`) and `extract_partial(loaded, crates)`. (b) `extract_partial_inner` rebuilds the three local maps (`crate_node_for`, `crate_name_for`, `module_node_for`) only for `crates`; every for-loop iterating `loaded.local_crates` now takes `crates` and short-circuits when `crate_for_def_id` is not in the dirty set. (c) `extract_bindings`, `extract_impl_items`, `extract_attributes`, `extract_signatures`, `extract_statics`, `extract_usages` take a `local_crates: &[Crate]` parameter. (d) `extract_usages` only emits references whose originating module is in a dirty crate. (e) Re-export `pub use self::extract::{extract_full, extract_partial};`.

VERIFY: existing extract tests pass; new `extract_partial_matches_full_on_subset` test asserts partial nodes are a subset of full nodes with identical bincode bytes for shared keys.

### Step 2 — Working snapshot + `WorkspaceHost::open_from_published`

WHERE: `crates/rmc-graph/src/host/working_snapshot.rs`, `host/mod.rs`. DEPENDS: Step 1.

(a) `WorkingSnapshot::init_from_base(base: &OpenedSnapshot, session_id: &str)`:
```rust
let working_dir = working_root().join(session_id);
fs::create_dir_all(&working_dir)?;
base.env.copy_to_path(working_dir.join("data.mdb"), CompactionOption::Disabled)?;
```

(b) `WorkspaceHost::open_from_published(workspace, base_graph_id, session_id)`:
1. `let base = open_specific(&paths, base_graph_id, GraphEnvOptions::default())?.ok_or(HostError::ColdRebuildRequired)?;`
2. `let working = WorkingSnapshot::init_from_base(&base, session_id)?;`
3. Open the freshly-copied working env:
   ```rust
   // SAFETY: heed's `EnvOpenOptions::open` is `unsafe` because LMDB maps the file
   // and assumes no other process mutates it concurrently with incompatible flags.
   // `working.dir` was just produced by `copy_to_path` into a private
   // `working/<session_id>/` directory owned by this host; no other process or
   // env handle has it open, and we open it `WithoutTls` for single-threaded
   // write access. Those are exactly the invariants LMDB requires for soundness.
   let env = Arc::new(unsafe {
       GraphEnvOptions::default().to_open_options().open(&working.dir)?
   });
   ```
4. `let dbs = { let rtxn = env.read_txn()?; GraphDatabases::open(&env, &rtxn)?.ok_or(HostError::ColdRebuildRequired)? };`
5. `let loaded = loader::load(workspace)?;` (slow path, once per host open). Populate `crate_target_kinds_by_name` / `crate_target_kinds_by_root_file` from `loaded` here.
6. `let mut analysis = AnalysisHost::with_database(loaded.db);` Keep `loaded.vfs`, `loaded.local_crates`, `loaded.workspace_root`.
7. Build `file_to_crate` by scanning `dbs.nodes_by_id` for `kind == Item`, grouping `Node.file → Node.crate_id`.
8. Build `crate_id_to_handle`: walk `local_crates`; compute NodeId via `NodeId::from_components(&[workspace_hash, "crate", crate_display_name(db, krate)])`; cross-reference with `nodes_by_id`.
9. Build `reverse_dep_index` once from `OpenedSnapshot::crate_edges` (used by Step 4); init `recent_file_prior_text` empty.

All `?` above propagate into `HostError` via the `#[from]`-style conversions
(`heed::Error → Storage`, `io::Error → Io`); `open_from_published` returns
`Result<WorkspaceHost, HostError>`.

VERIFY: `open_from_published_round_trips_node_count`.

### Step 3 — RA edit ingestion in `apply_edits`

WHERE: `crates/rmc-graph/src/host/edits.rs`. DEPENDS: Step 2.

`apply_edits` is **synchronous**: it holds the exclusive workspace lock for a
single non-`await` critical section, so the guard is never alive across a
suspension point (§12). (The async lock acquisition happens before the critical
section; if `lock_exclusive` must be async, acquire it in the caller / via a
sync `lock_exclusive_blocking` and pass the guard in, but do **not** `.await`
while holding it.) Resolve each `VfsPath` to a `file_id` exactly once and carry
it, so no `.expect("checked")` re-lookup is needed:

```rust
let _guard = self.locks.workspace_locks.lock_exclusive(&self.workspace_root);
let next_seq = self.edit_seq.next();

// Resolve once: (FileId, &FileEdit). Errors before mutating anything.
let mut resolved = Vec::with_capacity(edits.len());
for edit in edits {
    let abs = self.workspace_root.join(&edit.path);
    let vfs_path = ra_ap_vfs::VfsPath::new_real_path(abs.to_string_lossy().into_owned());
    let Some(file_id) = self.vfs.file_id(&vfs_path) else {
        return Err(HostError::UnknownFile(edit.path.clone()));
    };
    let prior = self.analysis.raw_database().file_text(file_id).to_string();
    self.recent_file_prior_text.entry(edit.path.clone()).or_insert(prior);
    resolved.push((file_id, edit));
}

let mut change = ra_ap_ide::Change::new();
for (file_id, edit) in &resolved {
    change.change_file(*file_id, Some(std::sync::Arc::from(edit.new_text.as_str())));
}
self.analysis.apply_change(change);
// …compute_patch / apply_patch run within the same critical section, then the
// guard drops. apply_edits as a whole takes no `.await`, so it is trivially
// cancellation-safe at this layer.
```

VERIFY: `apply_edits_invalidates_salsa`.

### Step 4 — Dirty-file → crate map + D2 affected-set

WHERE: `crates/rmc-graph/src/host/affected.rs`. DEPENDS: 1–3.

`affected_crates` is fallible: `CargoManifest` (and macro escalation) cannot be
served by the warm path, so it returns `Err(HostError::ColdRebuildRequired)`
rather than a bare `Vec` that the `return Err(...)` arm could never inhabit (§6).

```rust
pub(crate) fn affected_crates(
    host: &WorkspaceHost,
    edits: &[FileEdit],
) -> Result<Vec<NodeId>, HostError> {
    let mut dirty_directly: HashSet<NodeId> = HashSet::new();
    for edit in edits {
        let crates = host.file_to_crate.get(&edit.path).cloned().unwrap_or_default();
        if crates.is_empty() {
            dirty_directly.extend(host.fallback_crates_for_path(&edit.path));
        } else {
            dirty_directly.extend(crates);
        }
    }
    let class = edits
        .iter()
        .map(|e| e.edit_class)
        .max_by_key(class_severity)
        .unwrap_or(EditClass::BodyOnly);
    let affected = match class {
        EditClass::BodyOnly => dirty_directly.into_iter().collect(),
        EditClass::SignatureOrVis | EditClass::ItemAddRemove | EditClass::ModuleTree => {
            let reverse = &host.reverse_dep_index;   // memoised at host open
            let mut closure = dirty_directly.clone();
            let mut queue: Vec<_> = dirty_directly.into_iter().collect();
            while let Some(c) = queue.pop() {
                if let Some(rdeps) = reverse.get(&c) {
                    for &r in rdeps {
                        if closure.insert(r) { queue.push(r); }
                    }
                }
            }
            closure.into_iter().collect()
        }
        EditClass::Macro => full_workspace_crates(host),
        EditClass::CargoManifest => return Err(HostError::ColdRebuildRequired),
    };
    Ok(affected)
}
```

`reverse_dep_index` is built once at host open from `OpenedSnapshot::crate_edges`
(reversed) and stored as the `HashMap<NodeId, Vec<NodeId>>` field. Module-tree
edits invalidate and rebuild it.

VERIFY: `body_edit_does_not_expand_reverse_deps`, `sig_edit_does_expand`.

### Step 5 — Scoped re-extract

WHERE: `crates/rmc-graph/src/host/re_extract.rs`. DEPENDS: 1, 4.

```rust
pub(crate) fn re_extract(
    host: &WorkspaceHost,
    dirty: &[NodeId],
) -> Result<PartialExtractionModel, HostError> {
    let dirty_crates: Vec<Crate> = dirty.iter()
        .filter_map(|nid| host.crate_id_to_handle.get(nid).copied())
        .collect();
    let loaded_view = LoadedWorkspaceRef {
        workspace_root: &host.workspace_root,
        db: host.analysis.raw_database(),
        vfs: &host.vfs,
        local_crates: &dirty_crates,
        crate_target_kinds_by_name: &host.crate_target_kinds_by_name,
        crate_target_kinds_by_root_file: &host.crate_target_kinds_by_root_file,
    };
    Ok(extract::extract_partial(&loaded_view.to_loaded(), &dirty_crates))
}
```

`LoadedWorkspaceRef` is a borrowed mirror so we don't clone `RootDatabase`. If borrowck is painful, generalize `extract_partial` to a `LoadedAccess` trait.

VERIFY: `partial_extract_after_body_edit_matches_cold_subset`.

### Step 6 — Compute `DiffPatch`

WHERE: `crates/rmc-graph/src/host/diff_patch.rs`. DEPENDS: 5.

Every persisted-byte conversion is fallible: a key whose width is not 32 bytes
means LMDB corruption, **not** a local invariant, so it becomes
`HostError::CorruptRecord` rather than `unwrap()` (§9). `?` on the read txn and
on bincode propagate via the `#[from]`-style conversions.

```rust
let rtxn = self.env.read_txn()?;
// 6a. Existing primary records for the dirty crates only.
let mut existing_nodes: HashMap<NodeId, Node> = HashMap::new();
for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
    let (k, n) = entry?;                       // heed::Error -> HostError::Storage
    let is_dirty = n.crate_id.is_some_and(|c| partial.dirty_crates().contains(&c));
    if !is_dirty { continue; }
    let key: [u8; 32] = k.try_into().map_err(|_| HostError::CorruptRecord {
        sub_db: "nodes_by_id",
        detail: "node key not 32 bytes".to_owned(),
    })?;
    existing_nodes.insert(NodeId::from_bytes_arr(key), n);
}

// 6b. Set difference for nodes.
let new_node_ids: HashSet<NodeId> = partial.nodes().keys().copied().collect();
let old_node_ids: HashSet<NodeId> = existing_nodes.keys().copied().collect();
for &id in new_node_ids.difference(&old_node_ids) { patch.node_inserts.push(partial.nodes()[&id].clone()); }
for &id in old_node_ids.difference(&new_node_ids) { patch.node_removes.push(id); }
for &id in new_node_ids.intersection(&old_node_ids) {
    if bincode::serialize(&partial.nodes()[&id])? != bincode::serialize(&existing_nodes[&id])? {
        patch.node_updates.push(partial.nodes()[&id].clone());
    }
}
```

Bindings + usages: same set-difference on `BindingId`/`UsageId`. Existing IDs read via the right secondary for dirty crates (`bindings_by_from_module.iter_dup_of(&rtxn, mod_id.as_bytes())`, `usages_by_consumer.prefix(parent_module)`); apply the same `try_into → CorruptRecord` guard to every secondary key/value read. Cross-crate usages from a clean crate to a dirty crate remain valid in LMDB because content-addressed IDs don't change. `contains` / `signatures` / `statics`: per-dirty-NodeId diff. Returns `Ok(patch)`.

VERIFY: `diff_patch_is_empty_on_no_change`.

### Step 7 — Apply patch under write txn, record undo

WHERE: `crates/rmc-graph/src/host/diff_patch.rs::apply_patch`. DEPENDS: 6.

Strict ordering: deletes (secondaries first, then primary) → updates → inserts (primary first, then secondaries).

A primary record slated for removal that is absent under the write txn means the
patch desynced from storage — corruption, not a local invariant — so it becomes
`HostError::MissingRecord` instead of `.expect("removing nonexistent")` (§9).
`batch.push` / `batch.seq` go through `UndoBatch`'s accessors since fields are now
private.

```rust
let mut wtxn = self.env.write_txn()?;
let mut batch = UndoBatch::new(next_seq, patch.size_hint());

for bid in &patch.binding_removes {
    let prior = self.dbs.bindings_by_id.get(&wtxn, bid.as_bytes())?
        .ok_or(HostError::MissingRecord { sub_db: "bindings_by_id" })?;
    self.dbs.bindings_by_from_module.delete_one_duplicate(
        &mut wtxn, prior.from_module.as_bytes(), bid.as_bytes(),
    )?;
    batch.push(UndoOp::BindingByFromModuleInsert {
        key: *prior.from_module.as_bytes(), value: *bid.as_bytes(),
    });
    self.dbs.bindings_by_target.delete_one_duplicate(
        &mut wtxn, prior.target.as_bytes(), bid.as_bytes(),
    )?;
    batch.push(UndoOp::BindingByTargetInsert {
        key: *prior.target.as_bytes(), value: *bid.as_bytes(),
    });
    self.dbs.bindings_by_id.delete(&mut wtxn, bid.as_bytes())?;
    batch.push(UndoOp::BindingRemove { key: *bid.as_bytes(), prior });
}
// ... mirror for usages (three secondaries), children_by_parent, node_removes.

for node in &patch.node_updates {
    let prior = self.dbs.nodes_by_id.get(&wtxn, node.id.as_bytes())?;
    self.dbs.nodes_by_id.put(&mut wtxn, node.id.as_bytes(), node)?;
    batch.push(UndoOp::NodeUpsert { key: *node.id.as_bytes(), prior });
}

for node in &patch.node_inserts {
    self.dbs.nodes_by_id.put(&mut wtxn, node.id.as_bytes(), node)?;
    batch.push(UndoOp::NodeUpsert { key: *node.id.as_bytes(), prior: None });
}
for (bid, binding) in &patch.binding_inserts {
    self.dbs.bindings_by_id.put(&mut wtxn, bid.as_bytes(), binding)?;
    batch.push(UndoOp::BindingUpsert { key: *bid.as_bytes(), prior: None });
    self.dbs.bindings_by_from_module.put(&mut wtxn, binding.from_module.as_bytes(), bid.as_bytes())?;
    batch.push(UndoOp::BindingByFromModuleDelete {
        key: *binding.from_module.as_bytes(), value: *bid.as_bytes(),
    });
    // ... bindings_by_target ...
}
```

(`UndoBatch::new(seq, cap)` builds the batch with `Vec::with_capacity(cap)`,
keeping the `seq`/`ops` fields private.)

**Critical:** `delete_one_duplicate` is the heed 0.22 helper that positions the cursor on the (key, value) pair. `Database::delete` on a DUP_SORT db removes *every* dup for that key — wrong here. Highest-risk correctness item; covered by `dup_sort_secondary_delete` test.

### Step 8 — Counter / manifest updates

A meta counter that is present but not 8 bytes wide is corruption, so the
`try_into` becomes `CorruptRecord` (§9) — only a *missing* counter defaults to
zero. The `wtxn` is committed atomically; if any `?` above fired the txn is
dropped (rolled back) and `self.edit_seq` / `self.undo` are left untouched, which
is what keeps `apply_patch` cancellation-safe (see `# Cancellation` on the
signature).

```rust
let dn = patch.node_inserts.len() as i64 - patch.node_removes.len() as i64;
let db = patch.binding_inserts.len() as i64 - patch.binding_removes.len() as i64;
let du = patch.usage_inserts.len() as i64 - patch.usage_removes.len() as i64;

for (name, delta) in [("node_count", dn), ("binding_count", db), ("usage_count", du)] {
    let prior_bytes: [u8; 8] = match self.dbs.meta_by_key.get(&wtxn, name)? {
        Some(b) => b.try_into().map_err(|_| HostError::CorruptRecord {
            sub_db: "meta_by_key",
            detail: format!("counter {name} not 8 bytes"),
        })?,
        None => [0; 8],
    };
    let prior = i64::from_le_bytes(prior_bytes);
    let new = (prior + delta).max(0) as u64;
    self.dbs.meta_by_key.put(&mut wtxn, name, &new.to_le_bytes())?;
    batch.push(UndoOp::MetaCounter { name, prior_le_bytes: prior_bytes });
}

wtxn.commit()?;            // atomic; only here do in-memory markers advance
self.undo.push(batch);
self.edit_seq = next_seq;
Ok(())
```

On-disk `manifest.json` rewritten too; atomic via temp + `fs::rename`.

VERIFY: `meta_counters_match_inserts_minus_removes`.

### Step 9 — Host trusts caller's EditClass

WHERE: `crates/rmc-graph/src/host/edits.rs`. Host does NOT parse textual diff. Caller (P1.5) constructs `FileEdit { edit_class }` from its verb dispatch.

### Step 10 — P0.3 jj wrapper

WHERE: `crates/rmc-graph/src/host/rollback.rs`.

Both wrappers return `Result<_, HostError>`; `Command::output().await?` surfaces
spawn failures as `HostError::Io`, and a non-zero exit becomes `HostError::Jj`
with the captured stderr.

```rust
async fn jj_op_log_head(workspace_root: &Path) -> Result<String, HostError> {
    let out = Command::new("jj").current_dir(workspace_root)
        .args(["op", "log", "--no-graph", "-n", "1", "--template", r#"self.id().short() ++ "\n""#])
        .output().await?;                                  // io::Error -> HostError::Io
    if !out.status.success() {
        return Err(HostError::Jj(String::from_utf8_lossy(&out.stderr).into_owned()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

async fn jj_op_restore(workspace_root: &Path, op_id: &str) -> Result<(), HostError> {
    let out = Command::new("jj").current_dir(workspace_root)
        .args(["op", "restore", op_id]).output().await?;
    if !out.status.success() {
        return Err(HostError::Jj(String::from_utf8_lossy(&out.stderr).into_owned()));
    }
    Ok(())
}
```

`Checkpoint::take(host)` captures `(jj_op_log_head().await?, edit_seq, drain recent_file_prior_text)` and returns `Ok(Checkpoint { … })`. It takes `&mut WorkspaceHost` because it drains `recent_file_prior_text`.

VERIFY: `jj_op_id_captured_on_take`.

### Step 11 — Host restore path

WHERE: `crates/rmc-graph/src/host/rollback.rs::WorkspaceHost::rollback`. DEPENDS: 7, 10.

**Async hygiene (§12):** the only `.await` in `rollback` is `jj op restore`, an
external subprocess. The exclusive workspace lock guard must **not** be held
across it. Acquire the lock, run the jj restore, release; then take a *separate*
synchronous critical section for the RA + LMDB replay (neither of which awaits).
The two phases are independent: jj operates on the working copy on disk, the
LMDB/RA replay operates on this host's private state. `undo.pop()` returns a
`Result` (no `unwrap`), and a post-rollback divergence is **surfaced** as
`HostError::RollbackDiverged` after reopening from base, not swallowed by a
`tracing::warn!`.

```rust
pub async fn rollback(&mut self, ck: &Checkpoint) -> Result<(), HostError> {
    // --- Phase 1: source restore (the ONLY await). Guard scoped so it is NOT
    // held across `.await`: lock_exclusive is async-acquired, the restore awaits,
    // then the guard drops at the end of this block before any further work. ---
    {
        let _guard = self.locks.workspace_locks.lock_exclusive(&self.workspace_root).await;
        jj_op_restore(&self.workspace_root, ck.jj_op_id()).await?;
    } // guard released here, before the sync section below

    // --- Phase 2: RA host + LMDB replay. Fully synchronous, no `.await`; a
    // re-acquired guard (sync) covers the non-await critical section. ---
    let diverged = {
        let _guard = self.locks.workspace_locks.lock_exclusive_blocking(&self.workspace_root);

        // 2a. RA host: replay inverse set_file_text.
        let mut change = ra_ap_ide::Change::new();
        for (path, prior_text) in ck.file_prior_text() {
            let vfs_path = ra_ap_vfs::VfsPath::new_real_path(
                self.workspace_root.join(path).to_string_lossy().into_owned()
            );
            let Some(file_id) = self.vfs.file_id(&vfs_path) else { continue };
            change.change_file(file_id, Some(std::sync::Arc::from(prior_text.as_str())));
        }
        self.analysis.apply_change(change);

        // 2b. LMDB: pop undo batches > marker, replay inverses.
        let mut wtxn = self.env.write_txn()?;
        while self.undo.last().is_some_and(|top| top.seq() > ck.edit_seq_marker()) {
            let batch = self.undo.pop()?;          // -> HostError::UndoLogInconsistent, never panics
            for op in batch.ops().iter().rev() {
                self.apply_undo_op(&mut wtxn, op)?;
            }
        }
        wtxn.commit()?;
        self.edit_seq = ck.edit_seq_marker();

        // 2c. Divergence guard.
        self.is_diverged_from_expected(ck)?
    }; // sync guard released

    if diverged {
        tracing::warn!("undo replay diverged; reopening from base (slow path)");
        self.reopen_from_base(ck)?;               // rebuild working snapshot from base
        return Err(HostError::RollbackDiverged);  // surface, do not silently succeed
    }
    Ok(())
}
```

`apply_undo_op` matches `UndoOp` (NodeUpsert {prior: None} → delete; {prior: Some(n)} → put; DUP_SORT inserts → put pair; etc.).

`reopen_from_base` (private host method) is the slow bail-out: it drops the
working snapshot, `copy_to_path`s the base again, and re-runs the Step 2 open
sequence. `is_diverged_from_expected` (private) re-checks the meta counters (and,
under `debug_assertions`, a Merkle root over `nodes_by_id`) against the
checkpoint; it returns `Result<bool, HostError>` because it reads LMDB.

### Step 12 — Bench harness

WHERE: `crates/rmc-graph/benches/incremental_extract.rs`.

```rust
fn bench_body_only(c: &mut Criterion) {
    let workspace = corpus::large_100k_loc();
    let base = build_and_persist(&workspace, BuildOptions::default()).unwrap();
    let mut host = WorkspaceHost::open_from_published(&workspace, &base.graph_id, "bench-session").unwrap();
    let target_file = corpus::pick_body_target(&workspace);
    let original = std::fs::read_to_string(&target_file).unwrap();
    c.bench_function("body_only_edit", |b| {
        let mut alt = 0;
        b.iter(|| {
            let text = if alt % 2 == 0 { mutate_body(&original, alt) } else { original.clone() };
            host.apply_edits(&[FileEdit { path: target_file.clone(), new_text: text, edit_class: EditClass::BodyOnly }]).unwrap();
            alt += 1;
        });
    });
}
```

Classes: `body_only_edit` (< 500ms p95), `sig_edit_reverse_deps_5` (< 2s p95), `item_add_remove` (< 1s p95), `module_tree` (< 2s p95). Output: JSON per bench `{name, p50_ms, p95_ms, p99_ms, max_ms, dirty_crate_count, patch_size}`.

## Tests

- **`roundtrip_body_only`** (`tests/host_body_roundtrip.rs`). 5-crate, ~3k LOC fixture. Cold-build → snapshot `cold_pre`. Apply body edit via host; cold-rebuild → `cold_post`. For dirty crate: working LMDB == `cold_post` on every persisted record. Non-dirty == `cold_pre`.
- **`roundtrip_sig_change`** (`tests/host_sig_roundtrip.rs`). Same shape, sig change in a leaf crate with 2 consumers; affected set = 3 crates; LMDB == cold for all three.
- **`undo_replay_equiv`** (`tests/host_undo.rs`). Apply 3 edits; `Env::copy_to_path` to side dir. `Checkpoint::take` before; `rollback(ck)`. Re-snapshot; walk every sub-DB pair and assert byte equality including DUP_SORT iteration order.
- **`concurrent_rollouts`** (`tests/host_concurrent.rs`). Two `WorkspaceHost`s over disjoint working snapshots, both initialised from same base. 10 edits each from two tokio tasks. Neither sees the other's mutations; published base manifest unchanged.
- **`dup_sort_secondary_delete`** (`tests/host_dup_sort.rs`). Two distinct bindings sharing `from_module` (DUP_SORT same key). Remove only one. `bindings_by_from_module.iter_dup_of(...)` returns exactly 1 entry after (not 0, not 2).
- **`affected_set_reverse_deps`** (`tests/host_affected.rs`). A depends on B. `EditClass::BodyOnly` in B → affected = {B}. `EditClass::SignatureOrVis` → {A, B}.
- **`checkpoint_restore_source`** (`tests/host_jj.rs`). `jj init`; write file; describe; take checkpoint; edit + describe; rollback → file reverted, `jj log -r @` shows old description.
- **`bench_incremental_extract`** — Step 12.

## Open decisions / risks

- **RA salsa fan-out (#1 lethal).** Body edit in `core` may invalidate types in 100 reverse-deps. D2 says "Body class → editing crate only"; salsa recomputes lazily wherever a query touches stale memo. Mitigation: the differential test (`roundtrip_body_only`) — if cold-rebuild diverges from warm-host for a non-dirty crate, fan-out leaked. Deeper mitigation: don't query non-dirty crates during re-extract (partial extractor passes only dirty `Crate` handles).
- **Memory.** Warm `RootDatabase` + `Vfs` ≈ 500MB-1GB for 100k LOC. N concurrent rollouts × ~750MB. Mitigation: episode pool with bounded concurrency (start at 2), reuse hosts across episodes (rollback to base instead of dropping).
- **DUP_SORT delete fiddliness.** heed 0.22 exposes `Database::delete_one_duplicate(&mut wtxn, &key, &value)` — the only safe call. `Database::delete(&mut wtxn, &key)` removes *all* dups → corruption. The `dup_sort_secondary_delete` test is the sentinel.
- **proc-macro / build.rs edits.** Per D2 they escalate to Full re-extract of every reverse-dep. Route to cold rebuild like CargoManifest until measurements show partial is worth the complexity.
- **Restore divergence detection.** Counter-check now (Step 11.4). Stronger check: Merkle root over `nodes_by_id` post-rollback compared to checkpoint root; gated by `debug_assertions`.
- **`AnalysisHost::apply_change` vs raw `set_file_text`.** Use `ra_ap_ide::Change::change_file(file_id, Option<Arc<str>>)` via `AnalysisHost::apply_change`. Both invalidate the same salsa input.
- **`crate_target_kinds_by_root_file` invalidation.** Cached at host open. ModuleTree edits do NOT invalidate. CargoManifest re-runs `load_crate_target_kinds` on cold-rebuild path.
- **File path canonicalisation.** `FileEdit.path` workspace-relative; `file_to_crate` keys workspace-relative; VFS paths absolute. Convert at edge in `apply_edits`.
- **`Vfs.file_id` returns None for newly-created files.** ModuleTree edits adding new `.rs` files need `vfs.set_file_contents(..., Some(bytes))` first. P1.5e concern.
- **`recent_file_prior_text` size.** Bounded by `Σ file_size for files edited since last Checkpoint::take`. 50 × 5 × ~10KB = ~500KB live.


---

