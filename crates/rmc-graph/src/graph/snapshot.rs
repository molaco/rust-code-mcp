//! Snapshot lifecycle: staging → write → publish.
//!
//! `build_and_persist` is the high-level entry point: it loads, extracts,
//! computes a fingerprint, opens a new heed env in a staging dir, writes the
//! whole model in one transaction, writes manifest.json, then atomically swaps
//! the workspace's `CURRENT` pointer.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use heed::{Env, RoTxn, RwTxn, WithoutTls};
type GraphRoTxn<'e> = RoTxn<'e, WithoutTls>;
type GraphRwTxn<'e> = RwTxn<'e>;

use super::extract;
use super::ids::{BindingId, NodeId, UsageId};
use super::loader::{self, LoadedWorkspace};
use super::model::{Binding, ExtractionModel, Namespace, Usage};
use super::storage::{
    CURRENT_POINTER_FILENAME, GraphDatabases, GraphEnvOptions, GraphManifest, GraphPaths,
    SCHEMA_VERSION, compute_fingerprint, default_data_dir, graph_id_for, read_manifest,
    read_manifest_compatible, write_manifest,
};

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub force_rebuild: bool,
    pub data_dir_override: Option<PathBuf>,
    pub env: GraphEnvOptions,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            force_rebuild: false,
            data_dir_override: None,
            env: GraphEnvOptions::default(),
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
    pub reused: bool,
    pub snapshot_path: PathBuf,
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

fn compute_snapshot_identity(
    workspace_root: PathBuf,
    paths: GraphPaths,
) -> Result<SnapshotIdentity> {
    let fingerprint = compute_fingerprint(&workspace_root)?;
    Ok(snapshot_identity(workspace_root, paths, fingerprint))
}

fn compute_snapshot_identity_timed(
    workspace_root: PathBuf,
    paths: GraphPaths,
    timing: bool,
) -> Result<SnapshotIdentity> {
    if !timing {
        return compute_snapshot_identity(workspace_root, paths);
    }

    let t = std::time::Instant::now();
    let fingerprint = compute_fingerprint(&workspace_root)?;
    eprintln!(
        "build:   compute_fingerprint          {:>9.2?}",
        t.elapsed()
    );
    Ok(snapshot_identity(workspace_root, paths, fingerprint))
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
        reused: true,
        snapshot_path: identity.snapshot_dir.clone(),
    }))
}

pub fn build_and_persist(directory: &Path, options: BuildOptions) -> Result<BuildResult> {
    let timing = std::env::var_os("EXTRACT_TIMING").is_some();
    let workspace_root = canonical_workspace_root(directory)?;

    let preflight_identity = if options.force_rebuild {
        None
    } else {
        let paths = graph_paths_for_workspace(&workspace_root, &options);
        let identity = compute_snapshot_identity_timed(workspace_root.clone(), paths, timing)?;
        if let Some(result) = try_reuse_existing_snapshot(&identity)? {
            if timing {
                eprintln!("build:   reused existing snapshot");
            }
            return Ok(result);
        }
        Some(identity)
    };

    let t = std::time::Instant::now();
    let loaded = loader::load(&workspace_root)?;
    if timing {
        eprintln!(
            "build:   loader::load                 {:>9.2?}  ({} local crates)",
            t.elapsed(),
            loaded.local_crates.len()
        );
    }

    let identity = if let Some(identity) = preflight_identity {
        identity
    } else {
        let paths = graph_paths_for_workspace(&loaded.workspace_root, &options);
        compute_snapshot_identity_timed(loaded.workspace_root.clone(), paths, timing)?
    };
    identity.paths.ensure_dirs()?;

    if identity.snapshot_dir.exists() {
        fs::remove_dir_all(&identity.snapshot_dir)
            .with_context(|| format!("failed to clear stale {}", identity.snapshot_dir.display()))?;
    }
    fs::create_dir_all(&identity.snapshot_dir)?;

    let env = unsafe {
        options
            .env
            .to_open_options()
            .open(&identity.snapshot_dir)
            .with_context(|| format!("open heed env at {}", identity.snapshot_dir.display()))?
    };

    let model = extract::extract(&loaded);

    let t = std::time::Instant::now();
    let (node_count, binding_count, usage_count) = write_model(
        &env,
        options.env,
        &model,
        &identity.paths.workspace_hash,
        &identity.fingerprint,
        &identity.graph_id,
    )?;
    if timing {
        eprintln!(
            "build:   write_model (LMDB)           {:>9.2?}  ({} nodes, {} bindings, {} usages)",
            t.elapsed(),
            node_count,
            binding_count,
            usage_count
        );
    }

    let manifest = GraphManifest {
        graph_id: identity.graph_id.clone(),
        workspace_root: loaded.workspace_root.display().to_string(),
        workspace_hash: identity.paths.workspace_hash.clone(),
        fingerprint: identity.fingerprint.clone(),
        schema_version: SCHEMA_VERSION,
        created_at_unix: now_unix()?,
        node_count,
        binding_count,
        usage_count,
    };
    write_manifest(&identity.manifest_path, &manifest)?;

    publish_current(&identity.paths, &identity.graph_id)?;

    Ok(BuildResult {
        graph_id: identity.graph_id,
        workspace_root: identity.workspace_root,
        fingerprint: identity.fingerprint,
        node_count,
        binding_count,
        usage_count,
        reused: false,
        snapshot_path: identity.snapshot_dir,
    })
}

/// Lower-level entry for tests that already have a `LoadedWorkspace` in hand.
pub(crate) fn persist_loaded(
    loaded: &LoadedWorkspace,
    options: &BuildOptions,
) -> Result<BuildResult> {
    let paths = graph_paths_for_workspace(&loaded.workspace_root, options);
    paths.ensure_dirs()?;
    let identity = compute_snapshot_identity(loaded.workspace_root.clone(), paths)?;

    if identity.snapshot_dir.exists() {
        fs::remove_dir_all(&identity.snapshot_dir)?;
    }
    fs::create_dir_all(&identity.snapshot_dir)?;
    let env = unsafe { options.env.to_open_options().open(&identity.snapshot_dir)? };

    let model = extract::extract(loaded);
    let (node_count, binding_count, usage_count) = write_model(
        &env,
        options.env,
        &model,
        &identity.paths.workspace_hash,
        &identity.fingerprint,
        &identity.graph_id,
    )?;
    let manifest = GraphManifest {
        graph_id: identity.graph_id.clone(),
        workspace_root: loaded.workspace_root.display().to_string(),
        workspace_hash: identity.paths.workspace_hash.clone(),
        fingerprint: identity.fingerprint.clone(),
        schema_version: SCHEMA_VERSION,
        created_at_unix: now_unix()?,
        node_count,
        binding_count,
        usage_count,
    };
    write_manifest(&identity.manifest_path, &manifest)?;
    publish_current(&identity.paths, &identity.graph_id)?;

    Ok(BuildResult {
        graph_id: identity.graph_id,
        workspace_root: identity.workspace_root,
        fingerprint: identity.fingerprint,
        node_count,
        binding_count,
        usage_count,
        reused: false,
        snapshot_path: identity.snapshot_dir,
    })
}

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

    // 1. Nodes
    for node in model.nodes.values() {
        dbs.nodes_by_id
            .put(&mut wtxn, node.id.as_bytes(), node)
            .context("put node")?;
    }
    let node_count = model.nodes.len() as u64;

    // 2. Bindings + per-target/per-from-module indexes
    let mut binding_count: u64 = 0;
    for binding in &model.bindings {
        let bid = binding_id_for(binding);
        // primary record
        dbs.bindings_by_id
            .put(&mut wtxn, bid.as_bytes(), binding)
            .context("put binding")?;
        // index from_module → bid
        dbs.bindings_by_from_module
            .put(&mut wtxn, binding.from_module.as_bytes(), bid.as_bytes())?;
        // index target → bid
        dbs.bindings_by_target
            .put(&mut wtxn, binding.target.as_bytes(), bid.as_bytes())?;
        binding_count += 1;
    }

    // 3. Contains hierarchy
    for &(parent, child) in &model.contains {
        dbs.children_by_parent
            .put(&mut wtxn, parent.as_bytes(), child.as_bytes())?;
    }

    // 4. Usages + per-target/per-consumer indexes
    let mut usage_count: u64 = 0;
    for usage in &model.usages {
        let uid = usage_id_for(usage);
        dbs.usages_by_id
            .put(&mut wtxn, uid.as_bytes(), usage)
            .context("put usage")?;
        dbs.usages_by_target
            .put(&mut wtxn, usage.target.as_bytes(), uid.as_bytes())?;
        dbs.usages_by_consumer
            .put(&mut wtxn, usage.consumer_module.as_bytes(), uid.as_bytes())?;
        if let Some(consumer_fn) = usage.consumer_function {
            dbs.usages_by_consumer_function.put(
                &mut wtxn,
                consumer_fn.as_bytes(),
                uid.as_bytes(),
            )?;
        }
        usage_count += 1;
    }

    // 4b. Signatures (v9): one bincode-encoded FunctionSignature per
    // local function NodeId. No DUP_SORT — one entry per fn. No new
    // index tables; lookups are direct via `signatures_by_target.get(node_id)`.
    for (target, sig) in &model.signatures {
        dbs.signatures_by_target
            .put(&mut wtxn, target.as_bytes(), sig)
            .context("put signature")?;
    }

    // 4c. Static metadata (v10): one bincode-encoded StaticMetadata per
    // local `static` NodeId. No DUP_SORT — one entry per static. Lookups
    // are direct via `static_metadata_by_target.get(node_id)`.
    for (target, meta) in &model.statics {
        dbs.static_metadata_by_target
            .put(&mut wtxn, target.as_bytes(), meta)
            .context("put static metadata")?;
    }

    // 5. Meta
    dbs.meta_by_key
        .put(&mut wtxn, "workspace_hash", workspace_hash.as_bytes())?;
    dbs.meta_by_key
        .put(&mut wtxn, "fingerprint", fingerprint.as_bytes())?;
    dbs.meta_by_key
        .put(&mut wtxn, "graph_id", graph_id.as_bytes())?;
    dbs.meta_by_key
        .put(&mut wtxn, "schema_version", &SCHEMA_VERSION.to_le_bytes())?;
    dbs.meta_by_key
        .put(&mut wtxn, "node_count", &node_count.to_le_bytes())?;
    dbs.meta_by_key
        .put(&mut wtxn, "binding_count", &binding_count.to_le_bytes())?;
    dbs.meta_by_key
        .put(&mut wtxn, "usage_count", &usage_count.to_le_bytes())?;

    wtxn.commit().context("commit graph write txn")?;
    Ok((node_count, binding_count, usage_count))
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

fn now_unix() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs())
}

/// A read handle to a published snapshot.
pub struct OpenedSnapshot {
    pub manifest: GraphManifest,
    pub snapshot_dir: PathBuf,
    pub env: Env<WithoutTls>,
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
    let env = unsafe {
        env_opts
            .to_open_options()
            .open(&snapshot_dir)
            .with_context(|| format!("open heed env at {}", snapshot_dir.display()))?
    };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::model::{BindingKind, NodeKind};
    use std::path::Path;

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
            graph_id: graph_id.clone(),
            workspace_root: workspace.display().to_string(),
            workspace_hash: paths.workspace_hash.clone(),
            fingerprint,
            schema_version,
            created_at_unix: now_unix().unwrap(),
            node_count: 11,
            binding_count: 7,
            usage_count: 5,
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
    fn build_and_open_self_workspace() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let tempdir = tempfile::tempdir().unwrap();
        let opts = BuildOptions {
            data_dir_override: Some(tempdir.path().to_path_buf()),
            ..Default::default()
        };

        let result = build_and_persist(Path::new(manifest_dir), opts.clone()).unwrap();
        assert!(!result.reused, "first build should not be reused");
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
