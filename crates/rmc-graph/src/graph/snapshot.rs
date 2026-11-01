//! Snapshot lifecycle: staging → write → publish.
//!
//! `build_and_persist` is the high-level entry point: it hashes the tree by
//! package, loads, decides between a full and an incremental build
//! (`incremental.rs`), stages a new heed env — empty, or a compacted copy of
//! the published snapshot with the dirty crates' rows deleted — extracts and
//! writes crate by crate (one transaction per crate, memory given back
//! between them), writes manifest.json, then atomically swaps
//! the workspace's `CURRENT` pointer and removes the snapshots it superseded.
//! Staging, publish and prune run under the workspace's `BUILD.lock`
//! ([`lock_workspace_builds`]).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use heed::{CompactionOption, Env, RoTxn, RwTxn, WithoutTls};
type GraphRoTxn<'e> = RoTxn<'e, WithoutTls>;
type GraphRwTxn<'e> = RwTxn<'e>;

static OPENED_GRAPH_ENVS: OnceLock<Mutex<HashMap<PathBuf, CachedGraphEnv>>> = OnceLock::new();

#[derive(Clone)]
struct CachedGraphEnv {
    options: GraphEnvOptions,
    env: Weak<Env<WithoutTls>>,
}

use super::extract;
use super::ids::{BindingId, NodeId, UsageId};
use super::incremental::{self, BuildPlan};
use super::input_slice::{InputChanged, InputSlice};
use super::loader::{self, LoadedWorkspace};
#[cfg(test)]
use super::model::ExtractionModel;
use super::model::{Binding, Namespace, NodeKind, Usage};
use super::ownership;
use super::storage::{
    CURRENT_POINTER_FILENAME, GraphDatabases, GraphEnvOptions, GraphManifest,
    GraphPaths, SCHEMA_VERSION, default_data_dir, graph_id_for,
    read_manifest, read_manifest_compatible, write_manifest,
};
#[cfg(test)]
use super::storage::compute_fingerprint;
use super::usages::UsagesPass;

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub force_rebuild: bool,
    pub data_dir_override: Option<PathBuf>,
    pub env: GraphEnvOptions,
    /// Called after each crate of the build, once rust-analyzer has dropped
    /// that crate's caches, to give the freed pages back to the kernel.
    /// Which call does that depends on the allocator the binary links
    /// (`malloc_trim` against mimalloc is a no-op), so the binary passes it;
    /// `rmc-server` has `mcp::memory::release_free_memory`. `None`: the pages
    /// stay with the allocator for reuse by the next crate.
    pub release_free_memory: Option<fn() -> bool>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            force_rebuild: false,
            data_dir_override: None,
            env: GraphEnvOptions::default(),
            release_free_memory: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BuildResult {
    pub graph_id: String,
    pub workspace_root: PathBuf,
    pub fingerprint: String,
    pub node_count: u64,
    pub binding_count: u64,
    pub usage_count: u64,
    pub usage_counts: Option<super::model::UsageCounts>,
    pub reused: bool,
    pub kind: BuildKind,
    pub snapshot_path: PathBuf,
}

/// What a build did ([`incremental`](super::incremental) decides).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BuildKind {
    /// A snapshot of today's tree already existed.
    Reused,
    /// Every crate built into an empty snapshot.
    Full { reason: String },
    /// The published snapshot copied, these crates deleted and rebuilt.
    Incremental {
        changed_packages: Vec<String>,
        rebuilt_crates: Vec<String>,
    },
}

#[derive(Debug, Clone)]
struct SnapshotIdentity {
    workspace_root: PathBuf,
    paths: GraphPaths,
    fingerprint: String,
    graph_id: String,
    snapshot_dir: PathBuf,
    manifest_path: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct GraphSnapshotCleanupOptions {
    pub dry_run: bool,
    pub data_dir_override: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphSnapshotCleanupEntry {
    pub label: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphSnapshotCleanupReport {
    pub cleared: Vec<GraphSnapshotCleanupEntry>,
    pub errors: Vec<String>,
}

/// Clear the persisted graph snapshot directory for one workspace.
///
/// The workspace path is canonicalized when possible, matching the graph
/// builder/opening path policy. If canonicalization fails, the original path
/// is still hashed so callers can safely use this for nonexistent paths and
/// receive an empty report rather than an error.
pub fn clear_workspace_snapshots(
    workspace_root: &Path,
    options: GraphSnapshotCleanupOptions,
) -> GraphSnapshotCleanupReport {
    let canonical = fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());
    let base_dir = options.data_dir_override.unwrap_or_else(default_data_dir);
    let paths = GraphPaths::for_workspace_in(&base_dir, &canonical);
    let mut report = GraphSnapshotCleanupReport::default();
    clear_existing_graph_dir(
        "Hypergraph snapshot",
        "hypergraph snapshot",
        &paths.root_dir,
        options.dry_run,
        &mut report,
    );
    report
}

/// Clear the root directory that contains all persisted graph snapshots.
pub fn clear_all_workspace_snapshots(
    options: GraphSnapshotCleanupOptions,
) -> GraphSnapshotCleanupReport {
    let base_dir = options.data_dir_override.unwrap_or_else(default_data_dir);
    let mut report = GraphSnapshotCleanupReport::default();
    clear_existing_graph_dir(
        "All hypergraph snapshots",
        "hypergraph snapshots",
        &base_dir,
        options.dry_run,
        &mut report,
    );
    report
}

fn clear_existing_graph_dir(
    label: &str,
    error_label: &str,
    path: &Path,
    dry_run: bool,
    report: &mut GraphSnapshotCleanupReport,
) {
    if !path.exists() {
        return;
    }
    if dry_run {
        report.cleared.push(GraphSnapshotCleanupEntry {
            label: label.to_string(),
            path: path.to_path_buf(),
        });
        return;
    }
    match fs::remove_dir_all(path) {
        Ok(()) => report.cleared.push(GraphSnapshotCleanupEntry {
            label: label.to_string(),
            path: path.to_path_buf(),
        }),
        Err(error) => report
            .errors
            .push(format!("Failed to clear {}: {}", error_label, error)),
    }
}

/// Open the current published snapshot for a canonical workspace root.
pub fn open_current_for_workspace(workspace_root: &Path) -> Result<Option<OpenedSnapshot>> {
    let paths = GraphPaths::for_workspace(workspace_root);
    open_current(&paths, GraphEnvOptions::default())
}

fn graph_paths_for_workspace(workspace_root: &Path, options: &BuildOptions) -> GraphPaths {
    match &options.data_dir_override {
        Some(base) => GraphPaths::for_workspace_in(base, workspace_root),
        None => GraphPaths::for_workspace(workspace_root),
    }
}

fn canonical_workspace_root(directory: &Path) -> Result<PathBuf> {
    directory
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", directory.display()))
}

fn snapshot_identity(
    workspace_root: PathBuf,
    paths: GraphPaths,
    fingerprint: String,
) -> SnapshotIdentity {
    let graph_id = graph_id_for(&paths.workspace_hash, &fingerprint);
    let snapshot_dir = paths.snapshot_dir(&graph_id);
    let manifest_path = paths.manifest_path(&graph_id);

    SnapshotIdentity {
        workspace_root,
        paths,
        fingerprint,
        graph_id,
        snapshot_dir,
        manifest_path,
    }
}

fn try_reuse_existing_snapshot(identity: &SnapshotIdentity) -> Result<Option<BuildResult>> {
    if !identity.manifest_path.exists() {
        return Ok(None);
    }

    let manifest = read_manifest(&identity.manifest_path)?;
    if !identity.snapshot_dir.join("data.mdb").exists() {
        return Ok(None);
    }

    Ok(Some(BuildResult {
        graph_id: manifest.graph_id,
        workspace_root: identity.workspace_root.clone(),
        fingerprint: manifest.fingerprint,
        node_count: manifest.node_count,
        binding_count: manifest.binding_count,
        usage_count: manifest.usage_count,
        usage_counts: manifest.usage_counts,
        reused: true,
        kind: BuildKind::Reused,
        snapshot_path: identity.snapshot_dir.clone(),
    }))
}

fn reuse_identity(identity: &SnapshotIdentity) -> Result<SnapshotIdentity> {
    // A forced rebuild of the same contents is a new immutable instance.
    // Prefer CURRENT to the deterministic content-addressed fallback, which
    // may still be retained by an older reader after that forced rebuild.
    if let Ok(id) = fs::read_to_string(&identity.paths.current_pointer_path) {
        let id = id.trim();
        let manifest_path = identity.paths.manifest_path(id);
        if manifest_path.exists() {
            if read_manifest_compatible(&manifest_path)?.is_some_and(|manifest| manifest.fingerprint == identity.fingerprint) {
                let mut current = identity.clone();
                current.graph_id = id.to_owned();
                current.snapshot_dir = identity.paths.snapshot_dir(id);
                current.manifest_path = manifest_path;
                return Ok(current);
            }
        }
    }
    Ok(identity.clone())
}

pub fn build_and_persist(directory: &Path, options: BuildOptions) -> Result<BuildResult> {
    retry_build(|| match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build_attempt(directory, &options))) {
        Ok(result) => result,
        Err(payload) => match payload.downcast::<ra_ap_ide_db::base_db::salsa::Cancelled>() {
            Ok(cancelled) => Err(anyhow::Error::new(*cancelled).context("rust-analyzer cancelled graph build")),
            Err(payload) => std::panic::resume_unwind(payload),
        },
    })
}

fn retry_build<T>(mut build: impl FnMut() -> Result<T>) -> Result<T> {
    // Cargo may create Cargo.lock on the first load. Any changed input means
    // discarding RA as well as the staged graph; rehashing an old RA database
    // under the new tree's identity would publish a mixture of two versions.
    for attempt in 0..3 {
        match build() {
            Err(error) if attempt < 2 && (error.is::<InputChanged>() || matches!(
                error.downcast_ref::<ra_ap_ide_db::base_db::salsa::Cancelled>(),
                Some(ra_ap_ide_db::base_db::salsa::Cancelled::PropagatedPanic)
            )) => {
                // A failed parallel query can cancel a dependent query even
                // for unchanged inputs. Never continue that database or skip
                // tokens: discard the whole staged graph and reload RA.
                tracing::warn!(%error, "retry graph build with a fresh input slice and database");
                if std::env::var_os("EXTRACT_TIMING").is_some() {
                    eprintln!("build: retry {} after {error:#}", attempt + 1);
                }
            }
            result => return result,
        }
    }
    unreachable!("the final attempt returns its result")
}

fn build_attempt(directory: &Path, options: &BuildOptions) -> Result<BuildResult> {
    let timing = std::env::var_os("EXTRACT_TIMING").is_some();
    let workspace_root = canonical_workspace_root(directory)?;

    let paths = graph_paths_for_workspace(&workspace_root, &options);
    paths.ensure_dirs()?;
    // Serialize reuse, staging and publication alike. A reuse must not point
    // CURRENT at a snapshot another builder is still replacing or pruning.
    let _build_lock = lock_workspace_builds(&paths)?;
    let slice = InputSlice::for_build(&workspace_root, Some(paths.root_dir.clone()))?;
    let inputs = slice.inputs.clone();
    let mut identity = snapshot_identity(workspace_root.clone(), paths, inputs.fingerprint());
    if !options.force_rebuild {
        let reusable = reuse_identity(&identity)?;
        if let Some(result) = try_reuse_existing_snapshot(&reusable)? {
            if timing {
                eprintln!("build:   reused existing snapshot");
            }
            // The snapshot matching today's tree need not be the one CURRENT
            // names: build at X, edit and build Y, revert to X — X is still on
            // disk if a reader held it open when Y pruned. Returning "reused X"
            // while CURRENT stays at Y would make every query answer from Y.
            slice.validate(&workspace_root)?;
            publish_current_if_changed(&reusable.paths, &reusable.graph_id)?;
            return Ok(result);
        }
    }

    let t = std::time::Instant::now();
    // Unprimed: the per-crate build primes the crates it walks itself.
    let mut loaded = loader::load_unprimed(&workspace_root)?;
    if timing {
        eprintln!(
            "build:   loader::load                 {:>9.2?}  ({} local crates)",
            t.elapsed(),
            loaded.local_crates.len()
        );
    }
    slice.validate(&workspace_root)?;

    // Never unlink/rewrite an existing instance, especially CURRENT or an env
    // held by a reader. Fingerprint names contents; graph_id names an immutable
    // build instance. On a collision a fresh opaque id preserves both.
    if identity.snapshot_dir.exists() {
        static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        identity.graph_id = NodeId::from_components(&[
            &identity.graph_id, &std::process::id().to_string(), &nonce.to_string(), &serial.to_string(),
        ]).to_hex();
        identity.snapshot_dir = identity.paths.snapshot_dir(&identity.graph_id);
        identity.manifest_path = identity.paths.manifest_path(&identity.graph_id);
    }

    let plan = if options.force_rebuild {
        BuildPlan::Full {
            reason: "force_rebuild".to_string(),
        }
    } else {
        match open_current(&identity.paths, options.env) {
            // A base under today's own id is a leftover of an interrupted
            // build of this very tree (the reuse check above did not accept
            // it); it is about to be cleared, so it cannot be copied.
            Ok(Some(base)) if base.manifest.graph_id == identity.graph_id => BuildPlan::Full {
                reason: "the published snapshot is an unfinished build of this tree".to_string(),
            },
            Ok(base) => incremental::plan_build(base, &inputs, &loaded)?,
            Err(error) => BuildPlan::Full {
                reason: format!("cannot open the published snapshot: {error:#}"),
            },
        }
    };

    fs::create_dir(&identity.snapshot_dir)?;
    // Declared before the env, so the env is closed before failed staging is
    // removed. A rejected attempt must never become a reusable snapshot.
    let mut staged = StagedSnapshot(Some(identity.snapshot_dir.clone()));

    // What to (re)build, the base's crate → packages entries to keep, and
    // the report. The base is copied and closed before the new env opens.
    let t = std::time::Instant::now();
    let mut row_origins = serde_json::Map::new();
    let mut base_graph_id = None;
    let (dirty, mut crate_packages, kind) = match plan {
        BuildPlan::Full { reason } => {
            let groups = extract::crate_groups(&loaded, &loaded.local_crates)?;
            (groups, BTreeMap::new(), BuildKind::Full { reason })
        }
        BuildPlan::Incremental {
            base,
            dirty,
            changed_packages,
        } => {
            base_graph_id = Some(base.manifest.graph_id.clone());
            for key in base.manifest.crate_packages.keys() {
                let origin = base.manifest.extraction_context.as_ref()
                    .and_then(|c| c.get("row_origins")).and_then(|c| c.get(key)).cloned()
                    .unwrap_or_else(|| serde_json::json!({"unknown": true, "base_graph_id": base.manifest.graph_id}));
                row_origins.insert(key.clone(), origin);
            }
            let target = identity.snapshot_dir.join("data.mdb");
            base.env
                .copy_to_path(&target, CompactionOption::Enabled)
                .with_context(|| {
                    format!(
                        "copy snapshot {} to {}",
                        base.snapshot_dir.display(),
                        target.display()
                    )
                })?;
            let mut crate_packages = base.manifest.crate_packages.clone();
            for group in &dirty {
                crate_packages.remove(&group.name);
            }
            let kind = BuildKind::Incremental {
                changed_packages: changed_packages.into_iter().collect(),
                rebuilt_crates: dirty.iter().map(|g| g.name.clone()).collect(),
            };
            (dirty, crate_packages, kind)
        }
    };
    tracing::info!(workspace = %identity.workspace_root.display(), ?kind, "hypergraph build");
    if timing {
        eprintln!(
            "build:   plan + copy                  {:>9.2?}  ({kind:?})",
            t.elapsed()
        );
    }

    let env = unsafe {
        options
            .env
            .to_open_options()
            .open(&identity.snapshot_dir)
            .with_context(|| format!("open heed env at {}", identity.snapshot_dir.display()))?
    };

    let t = std::time::Instant::now();
    let dirty_crates: Vec<_> = dirty.iter().flat_map(|g| g.crates.iter().copied()).collect();
    let (node_count, binding_count, usage_count) = write_by_crate(
        &env,
        &mut loaded,
        &dirty_crates,
        options.release_free_memory,
        &identity.paths.workspace_hash,
        &identity.fingerprint,
        &identity.graph_id,
    )?;
    for group in &dirty {
        crate_packages.insert(
            group.name.clone(),
            incremental::crate_packages(&loaded, group, &inputs),
        );
    }
    if timing {
        eprintln!(
            "build:   extract + write by crate     {:>9.2?}  ({} nodes, {} bindings, {} usages)",
            t.elapsed(),
            node_count,
            binding_count,
            usage_count
        );
    }

    slice.validate(&workspace_root)?;
    let usage_counts = Some(count_usages(&env)?);
    let targets = loaded.local_crates.iter().map(|krate| {
        let key = super::identity::target_key(&loaded.db, &loaded.vfs, &loaded.workspace_root, *krate);
        let target = serde_json::json!({"crate_id": super::identity::crate_node_id(&identity.paths.workspace_hash, &key),
            "target_key": key, "edition": krate.edition(&loaded.db).to_string(),
            "cfg_options": format!("{:?}", krate.cfg(&loaded.db))});
        if dirty_crates.contains(krate) {
            row_origins.insert(key, serde_json::json!({"graph_id": identity.graph_id,
                "extractor_revision": env!("RMC_GRAPH_REVISION"), "ra_revision": env!("RMC_RA_REVISION"),
                "salsa_revision": env!("RMC_SALSA_REVISION"),
                "target": target}));
        }
        target
    }).collect::<Vec<_>>();
    row_origins.retain(|key, _| crate_packages.contains_key(key));
    let manifest = GraphManifest {
        extraction_context: Some(serde_json::json!({
            "extractor_revision": env!("RMC_GRAPH_REVISION"),
            "ra_revision": env!("RMC_RA_REVISION"),
            "salsa_revision": env!("RMC_SALSA_REVISION"),
            "features": "all", "all_targets": false, "set_test": false,
            "build_script_outputs": false, "proc_macro_server": false,
            "targets": targets, "row_origins": row_origins, "incremental_base": base_graph_id,
            "build": kind
        })),
        graph_id: identity.graph_id.clone(),
        workspace_root: loaded.workspace_root.display().to_string(),
        workspace_hash: identity.paths.workspace_hash.clone(),
        fingerprint: identity.fingerprint.clone(),
        schema_version: SCHEMA_VERSION,
        created_at_unix: now_unix()?,
        node_count,
        binding_count,
        usage_count,
        usage_counts: usage_counts.clone(),
        inputs: Some(inputs),
        crate_packages,
    };
    write_manifest(&identity.manifest_path, &manifest)?;

    publish_current(&identity.paths, &identity.graph_id)?;
    staged.0 = None;
    prune_superseded_snapshots(&identity.paths, &identity.graph_id);

    Ok(BuildResult {
        graph_id: identity.graph_id,
        workspace_root: identity.workspace_root,
        fingerprint: identity.fingerprint,
        node_count,
        binding_count,
        usage_count,
        usage_counts,
        reused: false,
        kind,
        snapshot_path: identity.snapshot_dir,
    })
}

struct StagedSnapshot(Option<PathBuf>);

fn count_usages(env: &Env<WithoutTls>) -> Result<super::model::UsageCounts> {
    let txn = env.read_txn()?;
    let dbs = GraphDatabases::open(env, &txn)?.context("graph tables after write")?;
    let mut counts = super::model::UsageCounts::default();
    let mut sites = std::collections::HashSet::new();
    for entry in dbs.usages_by_id.iter(&txn)? {
        let (_, usage) = entry?;
        counts.observations += 1;
        counts.expanded_observations += u64::from(usage.expansion.is_some());
        sites.insert((usage.consumer_module, usage.file, usage.start, usage.end));
    }
    counts.source_sites = sites.len() as u64;
    Ok(counts)
}

impl Drop for StagedSnapshot {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            if let Err(error) = fs::remove_dir_all(path) {
                tracing::warn!(path = %path.display(), %error, "remove rejected graph staging");
            }
        }
    }
}

/// The whole-workspace model written in one transaction — the reference the
/// per-crate build ([`write_by_crate`]) is compared against, so test-only
/// until the whole-model path goes.
#[cfg(test)]
fn write_model(
    env: &Env<WithoutTls>,
    _env_opts: GraphEnvOptions,
    model: &ExtractionModel,
    workspace_hash: &str,
    fingerprint: &str,
    graph_id: &str,
) -> Result<(u64, u64, u64)> {
    let mut wtxn = env.write_txn().context("open write txn")?;
    let dbs = GraphDatabases::create(env, &mut wtxn)?;

    // 1–4. Every row, each with its owning crate (v13, `ownership.rs`).
    let written = ownership::write_owned_rows(&mut wtxn, &dbs, model, &|_| true)?;
    // Only nodes can be skipped by a full write — a stub no binding names
    // has no owner and is not written. The count is what the manifest has
    // always reported, so a gap here is a node the snapshot lost.
    if written.nodes != model.nodes.len() as u64 {
        bail!(
            "full write put {} of the model's {} nodes; the rest have no owning crate",
            written.nodes,
            model.nodes.len()
        );
    }
    let counts = (written.nodes, written.bindings, written.usages);
    write_meta(
        &mut wtxn,
        &dbs,
        workspace_hash,
        fingerprint,
        graph_id,
        counts,
    )?;

    wtxn.commit().context("commit graph write txn")?;
    Ok(counts)
}

/// Build and write the crates of `dirty` one group at a time
/// (`extract::extract_by_crate`) into `env` — empty for a full build, a copy
/// of the base for an incremental one. One write transaction per group
/// deletes the rows the group owned (none in an empty env) and writes its
/// new ones; then the meta rows.
///
/// Returns the counts of the whole snapshot: distinct nodes, bindings and
/// usages in their tables.
fn write_by_crate(
    env: &Env<WithoutTls>,
    loaded: &mut LoadedWorkspace,
    dirty: &[ra_ap_hir::Crate],
    release_free_memory: Option<fn() -> bool>,
    workspace_hash: &str,
    fingerprint: &str,
    graph_id: &str,
) -> Result<(u64, u64, u64)> {
    let mut wtxn = env.write_txn().context("open write txn")?;
    let dbs = GraphDatabases::create(env, &mut wtxn)?;
    wtxn.commit().context("commit graph table creation")?;

    extract::extract_by_crate(
        loaded,
        dirty,
        UsagesPass::from_env(),
        release_free_memory,
        &mut |model, group| {
            let mut wtxn = env.write_txn().context("open write txn")?;
            ownership::delete_crate_rows(&mut wtxn, &dbs, group.id)?;
            let written = ownership::write_owned_rows(&mut wtxn, &dbs, model, &|c| c == group.id)?;
            // The whole-model write checks that every node was written. Here
            // nodes of other crates are left out on purpose, so the check is
            // that the group's own, the workspace node and every stub (all
            // named by the group's bindings — the only rows that create one)
            // made it.
            let expected = model
                .nodes
                .values()
                .filter(|n| match n.kind {
                    NodeKind::Workspace | NodeKind::ExternalSymbol => true,
                    NodeKind::Crate | NodeKind::Module | NodeKind::Item => {
                        n.crate_id == Some(group.id)
                    }
                })
                .count() as u64;
            if written.nodes != expected {
                bail!(
                    "crate `{}`: wrote {} of its {expected} nodes; the rest have no owning crate",
                    group.name,
                    written.nodes
                );
            }
            wtxn.commit()
                .with_context(|| format!("commit rows of crate `{}`", group.name))?;
            Ok(())
        },
    )?;

    let mut wtxn = env.write_txn().context("open write txn")?;
    // Read from the tables, not summed per group: an incremental build
    // writes only some groups, and the workspace node and the stubs are
    // written by every group that has them.
    let counts = (
        dbs.nodes_by_id.len(&wtxn)?,
        dbs.bindings_by_id.len(&wtxn)?,
        dbs.usages_by_id.len(&wtxn)?,
    );
    write_meta(
        &mut wtxn,
        &dbs,
        workspace_hash,
        fingerprint,
        graph_id,
        counts,
    )?;
    wtxn.commit().context("commit graph meta")?;
    Ok(counts)
}

fn write_meta(
    wtxn: &mut heed::RwTxn<'_>,
    dbs: &GraphDatabases,
    workspace_hash: &str,
    fingerprint: &str,
    graph_id: &str,
    (node_count, binding_count, usage_count): (u64, u64, u64),
) -> Result<()> {
    dbs.meta_by_key
        .put(wtxn, "workspace_hash", workspace_hash.as_bytes())?;
    dbs.meta_by_key
        .put(wtxn, "fingerprint", fingerprint.as_bytes())?;
    dbs.meta_by_key.put(wtxn, "graph_id", graph_id.as_bytes())?;
    dbs.meta_by_key
        .put(wtxn, "schema_version", &SCHEMA_VERSION.to_le_bytes())?;
    dbs.meta_by_key
        .put(wtxn, "node_count", &node_count.to_le_bytes())?;
    dbs.meta_by_key
        .put(wtxn, "binding_count", &binding_count.to_le_bytes())?;
    dbs.meta_by_key
        .put(wtxn, "usage_count", &usage_count.to_le_bytes())?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn persist_test_model(
    data_dir: &Path,
    model: &ExtractionModel,
    env_opts: GraphEnvOptions,
) -> Result<OpenedSnapshot> {
    persist_test_with(
        data_dir,
        &model.workspace_root,
        env_opts,
        |env, hash, fp, id| write_model(env, env_opts, model, hash, fp, id),
    )
}

/// [`persist_test_model`] through the per-crate build instead of a model.
#[cfg(test)]
pub(crate) fn persist_test_by_crate(
    data_dir: &Path,
    loaded: &mut LoadedWorkspace,
    env_opts: GraphEnvOptions,
) -> Result<OpenedSnapshot> {
    let workspace_root = loaded.workspace_root.clone();
    let all = loaded.local_crates.clone();
    persist_test_with(data_dir, &workspace_root, env_opts, |env, hash, fp, id| {
        write_by_crate(env, loaded, &all, None, hash, fp, id)
    })
}

#[cfg(test)]
fn persist_test_with(
    data_dir: &Path,
    workspace_root: &Path,
    env_opts: GraphEnvOptions,
    write: impl FnOnce(&Env<WithoutTls>, &str, &str, &str) -> Result<(u64, u64, u64)>,
) -> Result<OpenedSnapshot> {
    let paths = GraphPaths::for_workspace_in(data_dir, workspace_root);
    paths.ensure_dirs()
        .with_context(|| format!("create graph dirs under {}", paths.root_dir.display()))?;
    let identity = snapshot_identity(
        workspace_root.to_path_buf(),
        paths,
        "test-fingerprint".to_string(),
    );
    let _build_lock = lock_workspace_builds(&identity.paths)?;
    fs::create_dir_all(&identity.snapshot_dir)
        .with_context(|| format!("create snapshot dir {}", identity.snapshot_dir.display()))?;
    {
        let env = unsafe {
            env_opts
                .to_open_options()
                .open(&identity.snapshot_dir)
                .with_context(|| format!("open heed env at {}", identity.snapshot_dir.display()))?
        };
        let (node_count, binding_count, usage_count) = write(
            &env,
            &identity.paths.workspace_hash,
            &identity.fingerprint,
            &identity.graph_id,
        )?;
        let manifest = GraphManifest {
            extraction_context: None,
            graph_id: identity.graph_id.clone(),
            workspace_root: workspace_root.display().to_string(),
            workspace_hash: identity.paths.workspace_hash.clone(),
            fingerprint: identity.fingerprint.clone(),
            schema_version: SCHEMA_VERSION,
            created_at_unix: now_unix()?,
            node_count,
            binding_count,
            usage_count,
            usage_counts: Some(count_usages(&env)?),
            inputs: None,
            crate_packages: BTreeMap::new(),
        };
        write_manifest(&identity.manifest_path, &manifest)?;
    }
    publish_current(&identity.paths, &identity.graph_id)?;
    prune_superseded_snapshots(&identity.paths, &identity.graph_id);
    open_current(&identity.paths, env_opts)?.context("test snapshot was not published")
}

pub(crate) fn binding_id_for(binding: &Binding) -> BindingId {
    let ns = match binding.namespace {
        Namespace::Type => "T",
        Namespace::Value => "V",
    };
    BindingId::from_components(&[
        binding.from_module.to_hex().as_str(),
        ns,
        binding.visible_name.as_str(),
        binding.target.to_hex().as_str(),
    ])
}

pub(crate) fn usage_id_for(u: &Usage) -> UsageId {
    let cat = match u.category {
        crate::graph::model::UsageCategory::Read => "R",
        crate::graph::model::UsageCategory::Write => "W",
        crate::graph::model::UsageCategory::Test => "T",
        crate::graph::model::UsageCategory::Other => "O",
    };
    UsageId::from_components(&[
        u.target.to_hex().as_str(),
        u.consumer_module.to_hex().as_str(),
        u.file.as_str(),
        u.start.to_string().as_str(),
        u.end.to_string().as_str(),
        cat,
        format!("{:?}", u.relation).as_str(),
        u.consumer_item.map(|id| id.to_hex()).unwrap_or_default().as_str(),
        serde_json::to_string(&u.expansion).expect("serializable expansion provenance").as_str(),
    ])
}

fn publish_current(paths: &GraphPaths, graph_id: &str) -> Result<()> {
    // Atomic on POSIX: write to a temp file, then rename.
    let tmp = paths.root_dir.join(format!("{CURRENT_POINTER_FILENAME}.tmp"));
    fs::write(&tmp, graph_id.as_bytes())
        .with_context(|| format!("write tmp pointer {}", tmp.display()))?;
    fs::rename(&tmp, &paths.current_pointer_path).with_context(|| {
        format!(
            "rename {} → {}",
            tmp.display(),
            paths.current_pointer_path.display()
        )
    })?;
    Ok(())
}

/// [`publish_current`] unless `CURRENT` already names `graph_id` — the reuse
/// path runs on every `build_hypergraph` call, and the common case is a no-op
/// that should not rewrite the file.
fn publish_current_if_changed(paths: &GraphPaths, graph_id: &str) -> Result<()> {
    let current = fs::read_to_string(&paths.current_pointer_path).unwrap_or_default();
    if current.trim() == graph_id {
        return Ok(());
    }
    publish_current(paths, graph_id)
}

/// Serialise staging → publish → prune across every builder of one workspace.
///
/// Builders really do run side by side: `build_hypergraph` goes through the
/// server's SHARED analysis gate, so two agents building one workspace build
/// it at once, and examples/tests/a second daemon are other processes on the
/// same data dir. Without this lock the prune would remove a snapshot another
/// builder is still writing — and two builders with the same fingerprint
/// already wiped each other's staging dir via `remove_dir_all(snapshot_dir)`.
///
/// Advisory (`flock` / `LockFileEx`), released when the returned file drops —
/// on panic and on process death too, so a crashed builder leaves no stale
/// lock. Blocking: a second builder waits for the first one's write (seconds),
/// which is cheaper than both writing the same graph.
fn lock_workspace_builds(paths: &GraphPaths) -> Result<fs::File> {
    let path = paths.build_lock_path();
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("open build lock {}", path.display()))?;
    file.lock()
        .with_context(|| format!("take build lock {}", path.display()))?;
    Ok(file)
}

/// Remove every snapshot of this workspace except `keep` (the one just
/// published) and except those still open in this process. Returns what was
/// removed.
///
/// Why it exists: `graph_id` hashes the tree fingerprint and SCHEMA_VERSION,
/// so every edit-then-build and every schema bump writes a NEW directory, and
/// nothing removed the old one — 45 snapshots of a single workspace, 8 GiB,
/// were found on 2026-09-09. A superseded snapshot is never opened again: the
/// only way back to it is the reuse preflight after reverting the tree, and a
/// rebuild there is far cheaper than keeping every past tree state on disk.
///
/// PRECONDITION: the caller holds [`lock_workspace_builds`]. That, and only
/// that, guarantees no other builder is mid-write in one of these directories.
///
/// Open snapshots are skipped, not removed: a query may be holding the previous
/// graph's `OpenedSnapshot` while this build publishes. On Unix unlinking under
/// a live mmap is harmless but frees nothing until the handle closes; on Windows
/// it fails outright. A skipped one goes on the next publish, so the directory
/// holds at most CURRENT plus whatever was open at that moment. A reader that
/// read CURRENT just before the publish and opens the old id just after the
/// removal gets "no snapshot" / an open error once — transient, the next call
/// reads the new CURRENT.
///
/// Best effort: the new snapshot is already published, so a failed removal is
/// logged and retried by the next build rather than failing this one.
fn prune_superseded_snapshots(paths: &GraphPaths, keep: &str) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    let entries = match fs::read_dir(&paths.snapshots_dir) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(
                dir = %paths.snapshots_dir.display(),
                %error,
                "snapshot prune: cannot list snapshots"
            );
            return removed;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name() == keep || !path.is_dir() || snapshot_env_is_open(&path) {
            continue;
        }
        match fs::remove_dir_all(&path) {
            Ok(()) => removed.push(path),
            Err(error) => tracing::warn!(
                dir = %path.display(),
                %error,
                "snapshot prune: cannot remove superseded snapshot, next build retries"
            ),
        }
    }
    if !removed.is_empty() {
        tracing::info!(
            workspace = %paths.workspace_hash,
            removed = removed.len(),
            "snapshot prune: removed superseded snapshots"
        );
    }
    removed
}

/// Whether this process holds a live env for `snapshot_dir` — i.e. some
/// `OpenedSnapshot` of it is still alive. Keyed like [`open_cached_env`]
/// (canonical path); a dir that cannot be canonicalized cannot be in the cache.
fn snapshot_env_is_open(snapshot_dir: &Path) -> bool {
    let Ok(canonical) = snapshot_dir.canonicalize() else {
        return false;
    };
    let Some(cache) = OPENED_GRAPH_ENVS.get() else {
        return false;
    };
    let cache = cache.lock().expect("opened graph env cache poisoned");
    cache
        .get(&canonical)
        .is_some_and(|cached| cached.env.strong_count() > 0)
}

fn now_unix() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs())
}

/// A read handle to a published snapshot.
pub struct OpenedSnapshot {
    pub manifest: GraphManifest,
    pub snapshot_dir: PathBuf,
    pub env: Arc<Env<WithoutTls>>,
    pub dbs: GraphDatabases,
    // codemap caches — both populated lazily by graph::codemap helpers.
    span_index: OnceLock<HashMap<String, Vec<(u32, u32, NodeId)>>>,
    line_to_byte: Mutex<HashMap<String, Arc<Vec<u32>>>>,
}

impl OpenedSnapshot {
    pub fn read_txn(&self) -> Result<GraphRoTxn<'_>> {
        Ok(self.env.read_txn()?)
    }

    pub fn write_txn(&self) -> Result<GraphRwTxn<'_>> {
        Ok(self.env.write_txn()?)
    }

    /// Look up a node record by id.
    pub fn node(&self, txn: &GraphRoTxn<'_>, id: NodeId) -> Result<Option<super::model::Node>> {
        Ok(self.dbs.nodes_by_id.get(txn, id.as_bytes())?)
    }

    /// Lazily build a per-file flat span index of all Item nodes that have
    /// `Node.file` and `Node.span` set. Returned map keys are workspace-
    /// relative file paths (matching `Node.file`); values are
    /// `(start_byte, end_byte, NodeId)` sorted by `start_byte` ascending.
    ///
    /// Built once per `OpenedSnapshot` handle. At ~1,500 items the flat
    /// `Vec` + binary search is faster than a tree.
    pub(crate) fn span_index(&self) -> &HashMap<String, Vec<(u32, u32, NodeId)>> {
        self.span_index.get_or_init(|| {
            let mut by_file: HashMap<String, Vec<(u32, u32, NodeId)>> = HashMap::new();
            let rtxn = match self.env.read_txn() {
                Ok(t) => t,
                Err(_) => return by_file,
            };
            let iter = match self.dbs.nodes_by_id.iter(&rtxn) {
                Ok(i) => i,
                Err(_) => return by_file,
            };
            for entry in iter {
                let Ok((id_bytes, node)) = entry else { continue };
                if !matches!(node.kind, super::model::NodeKind::Item) {
                    continue;
                }
                let Some(ref file) = node.file else { continue };
                let Some((start, end)) = node.span else { continue };
                let mut nid_arr = [0u8; 32];
                nid_arr.copy_from_slice(id_bytes);
                by_file
                    .entry(file.clone())
                    .or_default()
                    .push((start, end, NodeId(nid_arr)));
            }
            for v in by_file.values_mut() {
                v.sort_by_key(|&(s, _, _)| s);
            }
            by_file
        })
    }

    /// On-demand line→byte offset table for `workspace_relative_file`.
    /// `Arc<Vec<u32>>` so the table can be returned by clone without
    /// holding the mutex across awaits. `Vec<u32>` element `i` is the
    /// byte offset where source-line `i + 1` begins (1-indexed, matching
    /// the chunker's `line_start`).
    pub(crate) fn line_to_byte(
        &self,
        workspace_relative_file: &str,
    ) -> std::io::Result<Arc<Vec<u32>>> {
        // fast path: already cached
        {
            let map = self
                .line_to_byte
                .lock()
                .expect("line_to_byte mutex poisoned");
            if let Some(v) = map.get(workspace_relative_file) {
                return Ok(v.clone());
            }
        }
        // build it: read file, compute newline-offset prefix table
        let abs = PathBuf::from(&self.manifest.workspace_root).join(workspace_relative_file);
        let bytes = std::fs::read(&abs)?;
        let mut offsets: Vec<u32> = Vec::with_capacity(64);
        offsets.push(0); // line 1 starts at byte 0
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'\n' {
                let next = i.saturating_add(1);
                if next <= u32::MAX as usize {
                    offsets.push(next as u32);
                }
            }
        }
        let arc = Arc::new(offsets);
        let mut map = self
            .line_to_byte
            .lock()
            .expect("line_to_byte mutex poisoned");
        map.insert(workspace_relative_file.to_string(), arc.clone());
        Ok(arc)
    }
}

pub fn open_current(paths: &GraphPaths, env: GraphEnvOptions) -> Result<Option<OpenedSnapshot>> {
    if !paths.current_pointer_path.exists() {
        return Ok(None);
    }
    let graph_id = fs::read_to_string(&paths.current_pointer_path)
        .with_context(|| format!("read {}", paths.current_pointer_path.display()))?;
    let graph_id = graph_id.trim().to_string();
    if graph_id.is_empty() {
        return Ok(None);
    }
    open_specific(paths, &graph_id, env)
}

pub(crate) fn open_specific(
    paths: &GraphPaths,
    graph_id: &str,
    env_opts: GraphEnvOptions,
) -> Result<Option<OpenedSnapshot>> {
    let snapshot_dir = paths.snapshot_dir(graph_id);
    let manifest_path = paths.manifest_path(graph_id);
    if !manifest_path.exists() {
        return Ok(None);
    }
    // Soft-fail on schema mismatch so callers see "no snapshot — call
    // build_hypergraph first" rather than a cryptic schema-mismatch error.
    let Some(manifest) = read_manifest_compatible(&manifest_path)? else {
        return Ok(None);
    };
    if !snapshot_dir.join("data.mdb").exists() {
        bail!("snapshot manifest exists but data.mdb missing at {}", snapshot_dir.display());
    }
    let env = open_cached_env(&snapshot_dir, env_opts)?;
    // Open dbs in a txn that we then COMMIT (not drop) — committing registers
    // the dbi handles back to the env so later txns can use them. Dropping
    // leaves the handles in a half-open state and later iter() returns EINVAL.
    let rtxn = env.read_txn()?;
    let dbs = GraphDatabases::open(&env, &rtxn)?
        .context("snapshot exists but databases not initialized")?;
    rtxn.commit()?;
    Ok(Some(OpenedSnapshot {
        manifest,
        snapshot_dir,
        env,
        dbs,
        span_index: OnceLock::new(),
        line_to_byte: Mutex::new(HashMap::new()),
    }))
}

fn open_cached_env(
    snapshot_dir: &Path,
    env_opts: GraphEnvOptions,
) -> Result<Arc<Env<WithoutTls>>> {
    let canonical_dir = snapshot_dir
        .canonicalize()
        .with_context(|| format!("canonicalize snapshot dir {}", snapshot_dir.display()))?;
    let cache = OPENED_GRAPH_ENVS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().expect("opened graph env cache poisoned");

    if let Some(cached) = cache.get(&canonical_dir) {
        if let Some(env) = cached.env.upgrade() {
            if cached.options != env_opts {
                bail!(
                    "snapshot env already open at {} with options {:?}; requested {:?}",
                    canonical_dir.display(),
                    cached.options,
                    env_opts
                );
            }
            return Ok(env);
        }
    }

    let env = unsafe {
        env_opts
            .to_open_options()
            .open(&canonical_dir)
            .with_context(|| format!("open heed env at {}", canonical_dir.display()))?
    };
    let env = Arc::new(env);
    cache.insert(
        canonical_dir,
        CachedGraphEnv {
            options: env_opts,
            env: Arc::downgrade(&env),
        },
    );
    Ok(env)
}

#[cfg(test)]
mod tests {
    #[test]
    fn propagated_query_failure_restarts_but_local_cancellation_does_not() {
        use ra_ap_ide_db::base_db::salsa::Cancelled;
        let mut attempts = 0;
        let result = super::retry_build(|| {
            attempts += 1;
            if attempts == 1 { Err(anyhow::Error::new(Cancelled::PropagatedPanic).context("a dependent query failed")) }
            else { Ok(42) }
        });
        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempts, 2);
        attempts = 0;
        let result: anyhow::Result<()> = super::retry_build(|| {
            attempts += 1;
            Err(anyhow::Error::new(Cancelled::PropagatedPanic))
        });
        assert!(result.unwrap_err().is::<Cancelled>());
        assert_eq!(attempts, 3);
        attempts = 0;
        let result: anyhow::Result<()> = super::retry_build(|| {
            attempts += 1;
            Err(anyhow::Error::new(Cancelled::Local))
        });
        assert!(result.unwrap_err().is::<Cancelled>());
        assert_eq!(attempts, 1);
    }
    use super::*;
    use crate::graph::model::{BindingKind, NodeKind};
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::Arc;

    thread_local! {
        static EDIT_DURING_BUILD: std::cell::RefCell<Option<(PathBuf, usize, usize)>> = const { std::cell::RefCell::new(None) };
    }

    fn edit_loaded_source() -> bool {
        EDIT_DURING_BUILD.with_borrow_mut(|edit| {
            if let Some((path, remaining, serial)) = edit {
                if *remaining > 0 {
                    *remaining -= 1;
                    *serial += 1;
                    fs::write(path, format!("pub fn changed_{serial}() {{}}\n")).unwrap();
                }
            }
        });
        false
    }

    #[test]
    fn changed_slice_reloads_and_failed_force_preserves_the_published_instance() {
        let work = tempfile::tempdir().unwrap();
        crate::graph::test_support::write_two_crate_fixture(work.path());
        let data = tempfile::tempdir().unwrap();
        let mut options = BuildOptions { data_dir_override: Some(data.path().to_owned()), ..Default::default() };
        // The fixture has no Cargo.lock: creating it must cause a reload,
        // then the very next build reuses the resulting immutable instance.
        let first = build_and_persist(work.path(), options.clone()).unwrap();
        assert!(work.path().join("Cargo.lock").exists());
        assert_eq!(build_and_persist(work.path(), options.clone()).unwrap().graph_id, first.graph_id);
        let paths = GraphPaths::for_workspace_in(data.path(), work.path());
        let old_reader = open_current(&paths, options.env).unwrap().unwrap();
        let source = work.path().join("beta/src/lib.rs");
        EDIT_DURING_BUILD.with_borrow_mut(|edit| *edit = Some((source.clone(), 1, 0)));
        options.force_rebuild = true;
        options.release_free_memory = Some(edit_loaded_source);
        let changed = build_and_persist(work.path(), options.clone()).unwrap();
        assert_ne!(changed.graph_id, first.graph_id);
        let changed_reader = open_current(&paths, options.env).unwrap().unwrap();
        assert!(changed_reader.lookup_by_qualified_name("beta::changed_1").unwrap().is_some());
        assert!(old_reader.lookup_by_qualified_name("beta::twice").unwrap().is_some());
        // Force the same contents while a reader holds the old instance.
        let forced = build_and_persist(work.path(), options.clone()).unwrap();
        assert_ne!(forced.graph_id, changed.graph_id);
        let reused = build_and_persist(work.path(), BuildOptions { force_rebuild: false, release_free_memory: None, ..options.clone() }).unwrap();
        assert!(reused.reused);
        assert_eq!(reused.graph_id, forced.graph_id);
        let before = snapshot_names(&paths);
        // Continuous writes exhaust retries; neither CURRENT nor any old
        // reader is damaged, and all rejected staging directories are gone.
        EDIT_DURING_BUILD.with_borrow_mut(|edit| *edit = Some((source, 100, 1)));
        assert!(build_and_persist(work.path(), options.clone()).unwrap_err().is::<InputChanged>());
        EDIT_DURING_BUILD.with_borrow_mut(|edit| *edit = None);
        assert_eq!(fs::read_to_string(&paths.current_pointer_path).unwrap().trim(), forced.graph_id);
        assert_eq!(snapshot_names(&paths), before);
        assert!(changed_reader.lookup_by_qualified_name("beta::changed_1").unwrap().is_some());
        let repaired = build_and_persist(work.path(), BuildOptions { release_free_memory: None, ..options }).unwrap();
        assert_ne!(repaired.graph_id, forced.graph_id);
    }

    #[test]
    fn clear_workspace_snapshots_dry_run_reports_without_removing() {
        let td = tempfile::tempdir().unwrap();
        let workspace = td.path().join("workspace");
        let graphs_root = td.path().join("graphs");
        fs::create_dir_all(&workspace).unwrap();
        let paths = GraphPaths::for_workspace_in(&graphs_root, &workspace);
        fs::create_dir_all(&paths.root_dir).unwrap();

        let report = clear_workspace_snapshots(
            &workspace,
            GraphSnapshotCleanupOptions {
                dry_run: true,
                data_dir_override: Some(graphs_root),
            },
        );

        assert!(paths.root_dir.exists());
        assert!(report.errors.is_empty());
        assert_eq!(report.cleared.len(), 1);
        assert_eq!(report.cleared[0].label, "Hypergraph snapshot");
        assert_eq!(report.cleared[0].path, paths.root_dir);
    }

    #[test]
    fn clear_workspace_snapshots_removes_workspace_root() {
        let td = tempfile::tempdir().unwrap();
        let workspace = td.path().join("workspace");
        let graphs_root = td.path().join("graphs");
        fs::create_dir_all(&workspace).unwrap();
        let paths = GraphPaths::for_workspace_in(&graphs_root, &workspace);
        fs::create_dir_all(&paths.root_dir).unwrap();

        let report = clear_workspace_snapshots(
            &workspace,
            GraphSnapshotCleanupOptions {
                dry_run: false,
                data_dir_override: Some(graphs_root),
            },
        );

        assert!(!paths.root_dir.exists());
        assert!(report.errors.is_empty());
        assert_eq!(report.cleared.len(), 1);
        assert_eq!(report.cleared[0].label, "Hypergraph snapshot");
    }

    #[test]
    fn clear_all_workspace_snapshots_removes_graph_root() {
        let td = tempfile::tempdir().unwrap();
        let graphs_root = td.path().join("graphs");
        fs::create_dir_all(graphs_root.join("workspace_hash")).unwrap();

        let report = clear_all_workspace_snapshots(GraphSnapshotCleanupOptions {
            dry_run: false,
            data_dir_override: Some(graphs_root.clone()),
        });

        assert!(!graphs_root.exists());
        assert!(report.errors.is_empty());
        assert_eq!(report.cleared.len(), 1);
        assert_eq!(report.cleared[0].label, "All hypergraph snapshots");
        assert_eq!(report.cleared[0].path, graphs_root);
    }

    fn create_non_cargo_workspace(root: &Path) {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), "pub fn marker() {}\n").unwrap();
    }

    fn write_fake_snapshot(
        workspace: &Path,
        data_dir: &Path,
        schema_version: u32,
        write_data_file: bool,
    ) -> (String, PathBuf) {
        let paths = GraphPaths::for_workspace_in(data_dir, workspace);
        let fingerprint = compute_fingerprint(workspace).unwrap();
        let graph_id = graph_id_for(&paths.workspace_hash, &fingerprint);
        let snapshot_dir = paths.snapshot_dir(&graph_id);
        fs::create_dir_all(&snapshot_dir).unwrap();
        if write_data_file {
            fs::write(snapshot_dir.join("data.mdb"), b"stub").unwrap();
        }
        let manifest = GraphManifest {
            extraction_context: None,
            graph_id: graph_id.clone(),
            workspace_root: workspace.display().to_string(),
            workspace_hash: paths.workspace_hash.clone(),
            fingerprint,
            schema_version,
            created_at_unix: now_unix().unwrap(),
            node_count: 11,
            binding_count: 7,
            usage_count: 5,
            usage_counts: None,
            inputs: None,
            crate_packages: BTreeMap::new(),
        };
        write_manifest(&paths.manifest_path(&graph_id), &manifest).unwrap();
        (graph_id, snapshot_dir)
    }

    #[test]
    fn preflight_reuse_returns_without_loading_workspace() {
        let td = tempfile::tempdir().unwrap();
        let workspace = td.path().join("not-cargo");
        let data_dir = td.path().join("graphs");
        create_non_cargo_workspace(&workspace);
        let (graph_id, snapshot_dir) =
            write_fake_snapshot(&workspace, &data_dir, SCHEMA_VERSION, true);

        let result = build_and_persist(&workspace, BuildOptions {
            data_dir_override: Some(data_dir),
            ..Default::default()
        })
        .unwrap();

        assert!(result.reused);
        assert_eq!(result.graph_id, graph_id);
        assert_eq!(result.node_count, 11);
        assert_eq!(result.binding_count, 7);
        assert_eq!(result.usage_count, 5);
        assert_eq!(result.snapshot_path, snapshot_dir);
    }

    #[test]
    fn preflight_force_rebuild_calls_loader() {
        let td = tempfile::tempdir().unwrap();
        let workspace = td.path().join("not-cargo");
        let data_dir = td.path().join("graphs");
        create_non_cargo_workspace(&workspace);
        write_fake_snapshot(&workspace, &data_dir, SCHEMA_VERSION, true);

        let error = build_and_persist(&workspace, BuildOptions {
            force_rebuild: true,
            data_dir_override: Some(data_dir),
            ..Default::default()
        })
        .unwrap_err();

        assert!(error.to_string().contains("failed to load workspace"));
    }

    #[test]
    fn preflight_missing_manifest_calls_loader() {
        let td = tempfile::tempdir().unwrap();
        let workspace = td.path().join("not-cargo");
        create_non_cargo_workspace(&workspace);

        let error = build_and_persist(&workspace, BuildOptions {
            data_dir_override: Some(td.path().join("graphs")),
            ..Default::default()
        })
        .unwrap_err();

        assert!(error.to_string().contains("failed to load workspace"));
    }

    #[test]
    fn preflight_fingerprint_change_calls_loader() {
        let td = tempfile::tempdir().unwrap();
        let workspace = td.path().join("not-cargo");
        let data_dir = td.path().join("graphs");
        create_non_cargo_workspace(&workspace);
        write_fake_snapshot(&workspace, &data_dir, SCHEMA_VERSION, true);
        fs::write(workspace.join("src/lib.rs"), "pub fn changed() {}\n").unwrap();

        let error = build_and_persist(&workspace, BuildOptions {
            data_dir_override: Some(data_dir),
            ..Default::default()
        })
        .unwrap_err();

        assert!(error.to_string().contains("failed to load workspace"));
    }

    #[test]
    fn preflight_incompatible_manifest_errors_before_loader() {
        let td = tempfile::tempdir().unwrap();
        let workspace = td.path().join("not-cargo");
        let data_dir = td.path().join("graphs");
        create_non_cargo_workspace(&workspace);
        write_fake_snapshot(&workspace, &data_dir, SCHEMA_VERSION - 1, true);

        let error = build_and_persist(&workspace, BuildOptions {
            data_dir_override: Some(data_dir),
            ..Default::default()
        })
        .unwrap_err();

        assert!(error.to_string().contains("schema_version"));
    }

    #[test]
    fn preflight_missing_data_file_calls_loader() {
        let td = tempfile::tempdir().unwrap();
        let workspace = td.path().join("not-cargo");
        let data_dir = td.path().join("graphs");
        create_non_cargo_workspace(&workspace);
        write_fake_snapshot(&workspace, &data_dir, SCHEMA_VERSION, false);

        let error = build_and_persist(&workspace, BuildOptions {
            data_dir_override: Some(data_dir),
            ..Default::default()
        })
        .unwrap_err();

        assert!(error.to_string().contains("failed to load workspace"));
    }

    #[test]
    fn open_current_reuses_live_snapshot_env_handle() {
        let td = tempfile::tempdir().unwrap();
        let workspace = td.path().join("workspace");
        let data_dir = td.path().join("graphs");
        fs::create_dir_all(&workspace).unwrap();

        let model = ExtractionModel {
            workspace_root: workspace.clone(),
            workspace_hash: "test-workspace".to_string(),
            workspace_id: NodeId::from_components(&["workspace"]),
            nodes: BTreeMap::new(),
            bindings: Vec::new(),
            usages: Vec::new(),
            unresolved_calls: Vec::new(),
            contains: Vec::new(),
            signatures: Vec::new(),
            statics: Vec::new(),
        };
        let snap = persist_test_model(data_dir.as_path(), &model, GraphEnvOptions::default())
            .expect("persist test model");
        let paths = GraphPaths::for_workspace_in(&data_dir, &workspace);

        let reopened = open_current(&paths, GraphEnvOptions::default())
            .expect("open current")
            .expect("snapshot present");

        assert!(Arc::ptr_eq(&snap.env, &reopened.env));
    }

    fn empty_model(workspace: &Path) -> ExtractionModel {
        ExtractionModel {
            workspace_root: workspace.to_path_buf(),
            workspace_hash: "test-workspace".to_string(),
            workspace_id: NodeId::from_components(&["workspace"]),
            nodes: BTreeMap::new(),
            bindings: Vec::new(),
            usages: Vec::new(),
            unresolved_calls: Vec::new(),
            contains: Vec::new(),
            signatures: Vec::new(),
            statics: Vec::new(),
        }
    }

    fn snapshot_names(paths: &GraphPaths) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&paths.snapshots_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn prune_removes_every_snapshot_but_the_kept_one() {
        let td = tempfile::tempdir().unwrap();
        let paths = GraphPaths::for_workspace_in(td.path(), &td.path().join("ws"));
        for id in ["old1", "old2", "keep"] {
            fs::create_dir_all(paths.snapshot_dir(id)).unwrap();
            fs::write(paths.snapshot_dir(id).join("data.mdb"), b"x").unwrap();
        }

        let removed = prune_superseded_snapshots(&paths, "keep");

        assert_eq!(removed.len(), 2);
        assert_eq!(snapshot_names(&paths), vec!["keep".to_string()]);
    }

    #[test]
    fn prune_skips_a_snapshot_open_in_this_process_until_it_closes() {
        let td = tempfile::tempdir().unwrap();
        let workspace = td.path().join("workspace");
        let data_dir = td.path().join("graphs");
        fs::create_dir_all(&workspace).unwrap();
        let snap = persist_test_model(
            &data_dir,
            &empty_model(&workspace),
            GraphEnvOptions::default(),
        )
        .unwrap();
        let open_id = snap.manifest.graph_id.clone();
        let paths = GraphPaths::for_workspace_in(&data_dir, &workspace);
        fs::create_dir_all(paths.snapshot_dir("stale")).unwrap();

        // A newer graph got published while `snap` is still being read.
        let removed = prune_superseded_snapshots(&paths, "newer");
        assert_eq!(removed, vec![paths.snapshot_dir("stale")]);
        assert_eq!(snapshot_names(&paths), vec![open_id.clone()]);

        drop(snap);
        let removed = prune_superseded_snapshots(&paths, "newer");
        assert_eq!(removed, vec![paths.snapshot_dir(&open_id)]);
        assert!(snapshot_names(&paths).is_empty());
    }

    #[test]
    fn publishing_a_snapshot_prunes_the_superseded_ones() {
        let td = tempfile::tempdir().unwrap();
        let workspace = td.path().join("workspace");
        let data_dir = td.path().join("graphs");
        fs::create_dir_all(&workspace).unwrap();
        let paths = GraphPaths::for_workspace_in(&data_dir, &workspace);
        fs::create_dir_all(paths.snapshot_dir("from-an-older-tree")).unwrap();

        let snap = persist_test_model(
            &data_dir,
            &empty_model(&workspace),
            GraphEnvOptions::default(),
        )
        .unwrap();

        assert_eq!(snapshot_names(&paths), vec![snap.manifest.graph_id.clone()]);
    }

    #[test]
    fn reuse_points_current_at_the_reused_snapshot() {
        let td = tempfile::tempdir().unwrap();
        let workspace = td.path().join("not-cargo");
        let data_dir = td.path().join("graphs");
        create_non_cargo_workspace(&workspace);
        let (graph_id, _) = write_fake_snapshot(&workspace, &data_dir, SCHEMA_VERSION, true);
        let paths = GraphPaths::for_workspace_in(&data_dir, &workspace.canonicalize().unwrap());
        // The tree went X → Y → back to X: CURRENT still names Y.
        fs::write(&paths.current_pointer_path, "graph-of-the-edited-tree").unwrap();

        let result = build_and_persist(&workspace, BuildOptions {
            data_dir_override: Some(data_dir),
            ..Default::default()
        })
        .unwrap();

        assert!(result.reused);
        assert_eq!(
            fs::read_to_string(&paths.current_pointer_path).unwrap(),
            graph_id
        );
    }

    #[test]
    fn build_lock_is_exclusive_and_released_on_drop() {
        let td = tempfile::tempdir().unwrap();
        let paths = GraphPaths::for_workspace_in(td.path(), &td.path().join("ws"));
        paths.ensure_dirs().unwrap();
        let rival = || {
            fs::OpenOptions::new()
                .write(true)
                .open(paths.build_lock_path())
                .unwrap()
        };

        let held = lock_workspace_builds(&paths).unwrap();
        assert!(matches!(
            rival().try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));

        drop(held);
        rival().try_lock().unwrap();
    }

    #[test]
    fn build_and_open_self_workspace() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let tempdir = tempfile::tempdir().unwrap();
        let opts = BuildOptions {
            data_dir_override: Some(tempdir.path().to_path_buf()),
            ..Default::default()
        };
        // A snapshot left by an earlier tree state: the build must prune it.
        let stale_paths = GraphPaths::for_workspace_in(
            tempdir.path(),
            &Path::new(manifest_dir).canonicalize().unwrap(),
        );
        let stale = stale_paths.snapshot_dir("from-an-older-tree");
        fs::create_dir_all(&stale).unwrap();

        let result = build_and_persist(Path::new(manifest_dir), opts.clone()).unwrap();
        assert!(!result.reused, "first build should not be reused");
        assert!(
            !stale.exists(),
            "build_and_persist must prune superseded snapshots"
        );
        assert!(result.node_count > 0);
        assert!(result.binding_count > 0);
        assert!(
            result.usage_count > 0,
            "Phase 2 must populate at least one usage"
        );
        assert!(result.snapshot_path.join("data.mdb").exists());

        // Reuse path: second call should return reused=true with no rebuild.
        let result2 = build_and_persist(Path::new(manifest_dir), opts.clone()).unwrap();
        assert!(result2.reused);
        assert_eq!(result.graph_id, result2.graph_id);
        assert_eq!(result.node_count, result2.node_count);

        // Open and read back.
        let paths = GraphPaths::for_workspace_in(tempdir.path(), &result.workspace_root);
        let opened = open_current(&paths, GraphEnvOptions::default())
            .unwrap()
            .expect("snapshot opens via CURRENT pointer");

        let rtxn = opened.read_txn().unwrap();

        // Find the loader::load function NodeId by scanning nodes_by_id.
        let mut load_fn_id: Option<NodeId> = None;
        for entry in opened.dbs.nodes_by_id.iter(&rtxn).unwrap() {
            let (key, node) = entry.unwrap();
            if node.kind == NodeKind::Item
                && node.qualified_name == "rmc_graph::graph::loader::load"
            {
                let mut id = [0u8; 32];
                id.copy_from_slice(key);
                load_fn_id = Some(NodeId(id));
                break;
            }
        }
        let load_fn_id = load_fn_id.expect("load fn node persisted");

        // bindings_by_target should yield at least one BindingId for it
        // (the `pub use loader::load` in graph/mod.rs).
        let mut hits = 0;
        for entry in opened
            .dbs
            .bindings_by_target
            .iter(&rtxn)
            .unwrap()
            .take(1_000_000)
        {
            let (k, _v) = entry.unwrap();
            if k == load_fn_id.as_bytes() {
                hits += 1;
            }
        }
        assert!(hits > 0, "expected at least one binding targeting load fn");

        // usages_by_target: load fn is referenced from at least one local module
        // (the build_and_persist call inside this very test, plus the lib API
        // surface). Just assert ≥1 usage.
        let mut usage_hits = 0;
        for entry in opened.dbs.usages_by_target.iter(&rtxn).unwrap() {
            let (k, _v) = entry.unwrap();
            if k == load_fn_id.as_bytes() {
                usage_hits += 1;
            }
        }
        assert!(
            usage_hits > 0,
            "expected at least one usage targeting load fn"
        );

        // usages_by_id: at least one record round-trips with sane fields.
        let sample = opened
            .dbs
            .usages_by_id
            .iter(&rtxn)
            .unwrap()
            .next()
            .expect("at least one usage persisted")
            .unwrap();
        let (_uid, usage) = sample;
        assert!(usage.start <= usage.end, "usage range must be ordered");
        assert!(!usage.file.is_empty(), "usage file path must be non-empty");
        assert!(
            !usage.file.starts_with('/'),
            "usage file must be workspace-relative, got {}",
            usage.file
        );

        // children_by_parent: workspace node should have at least the crate as child.
        let workspace_id = opened
            .dbs
            .nodes_by_id
            .iter(&rtxn)
            .unwrap()
            .find_map(|e| {
                let (k, n) = e.unwrap();
                if n.kind == NodeKind::Workspace {
                    let mut id = [0u8; 32];
                    id.copy_from_slice(k);
                    Some(NodeId(id))
                } else {
                    None
                }
            })
            .expect("workspace node");
        let mut workspace_children = 0;
        for entry in opened.dbs.children_by_parent.iter(&rtxn).unwrap() {
            let (k, _v) = entry.unwrap();
            if k == workspace_id.as_bytes() {
                workspace_children += 1;
            }
        }
        assert!(workspace_children >= 1, "workspace should contain ≥1 crate");

        // Visit at least one Declared binding via bindings_by_id.
        let mut declared = 0;
        for entry in opened.dbs.bindings_by_id.iter(&rtxn).unwrap() {
            let (_k, b) = entry.unwrap();
            if b.kind == BindingKind::Declared {
                declared += 1;
                if declared > 5 {
                    break;
                }
            }
        }
        assert!(declared > 0, "should have ≥1 declared binding persisted");
    }
}
