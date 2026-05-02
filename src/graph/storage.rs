//! Storage layout, heed environment, and database schema for graph snapshots.
//!
//! Filesystem layout (lifted from the prior attempt — this part worked):
//!
//! ```text
//! <data_dir>/graphs/<workspace_hash>/
//!   CURRENT                ← text file with active graph_id (hex)
//!   snapshots/
//!     <graph_id>/
//!       data.mdb           ← heed env
//!       lock.mdb
//!       manifest.json      ← debug/operational metadata
//! ```
//!
//! The schema is **hash-only-keyed**. Every secondary index uses a 32-byte
//! NodeId/BindingId as both key and value (well under LMDB's 511-byte limit).
//! Human-readable names live inside the bincode-serialized record values.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use heed::types::{Bytes, SerdeBincode, Str, Unit};
use heed::{Database, Env, EnvOpenOptions, RoTxn, RwTxn, WithoutTls};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use super::ids::{BindingId, NodeId};
use super::model::{Binding, Node, Usage};

// v2 (2026-05): added usages_by_id / usages_by_target / usages_by_consumer
// sub-databases and `usage_count` to the manifest. v1 graph_ids and v2 graph_ids
// are disjoint (graph_id_for hashes SCHEMA_VERSION), so old snapshots simply
// stop being reused; they remain on disk until a manual cleanup.
pub const SCHEMA_VERSION: u32 = 2;
pub const CURRENT_POINTER_FILENAME: &str = "CURRENT";
pub const SNAPSHOTS_DIRNAME: &str = "snapshots";
pub const MANIFEST_FILENAME: &str = "manifest.json";

const DEFAULT_MAP_SIZE: usize = 1 << 30; // 1 GiB
const DEFAULT_MAX_DBS: u32 = 16;
const DEFAULT_MAX_READERS: u32 = 256;

#[derive(Debug, Clone, Copy)]
pub struct GraphEnvOptions {
    pub map_size: usize,
    pub max_dbs: u32,
    pub max_readers: u32,
}

impl Default for GraphEnvOptions {
    fn default() -> Self {
        Self {
            map_size: DEFAULT_MAP_SIZE,
            max_dbs: DEFAULT_MAX_DBS,
            max_readers: DEFAULT_MAX_READERS,
        }
    }
}

impl GraphEnvOptions {
    pub fn to_open_options(self) -> EnvOpenOptions<WithoutTls> {
        let mut options = EnvOpenOptions::new().read_txn_without_tls();
        options.map_size(self.map_size);
        options.max_dbs(self.max_dbs);
        options.max_readers(self.max_readers);
        options
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphPaths {
    pub workspace_hash: String,
    pub root_dir: PathBuf,
    pub current_pointer_path: PathBuf,
    pub snapshots_dir: PathBuf,
}

impl GraphPaths {
    pub fn for_workspace(workspace_root: &Path) -> Self {
        Self::for_workspace_in(&default_data_dir(), workspace_root)
    }

    pub fn for_workspace_in(base_dir: &Path, workspace_root: &Path) -> Self {
        let workspace_hash = super::ids::workspace_hash(workspace_root);
        let root_dir = base_dir.join(&workspace_hash);
        Self {
            current_pointer_path: root_dir.join(CURRENT_POINTER_FILENAME),
            snapshots_dir: root_dir.join(SNAPSHOTS_DIRNAME),
            workspace_hash,
            root_dir,
        }
    }

    pub fn snapshot_dir(&self, graph_id: &str) -> PathBuf {
        self.snapshots_dir.join(graph_id)
    }

    pub fn manifest_path(&self, graph_id: &str) -> PathBuf {
        self.snapshot_dir(graph_id).join(MANIFEST_FILENAME)
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.snapshots_dir)
    }
}

pub fn default_data_dir() -> PathBuf {
    ProjectDirs::from("dev", "rust-code-mcp", "search")
        .map(|dirs| dirs.data_dir().join("graphs"))
        .unwrap_or_else(|| PathBuf::from(".rust-code-mcp").join("graphs"))
}

/// Hash inputs that determine whether a snapshot is still current.
/// For v1: Cargo.toml + Cargo.lock + every `.rs` file under the workspace,
/// excluding `target/`. Kept simple — change detection is a rebuild trigger,
/// nothing more.
pub fn compute_fingerprint(workspace_root: &Path) -> Result<String> {
    let mut entries: Vec<(String, [u8; 32])> = Vec::new();

    for entry in WalkDir::new(workspace_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            !e.path()
                .components()
                .any(|c| c.as_os_str() == "target" || c.as_os_str() == ".git")
        })
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if !entry.file_type().is_file() {
            continue;
        }
        let interesting = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|ext| ext == "rs")
            .unwrap_or(false)
            || matches!(
                path.file_name().and_then(|s| s.to_str()),
                Some("Cargo.toml") | Some("Cargo.lock")
            );
        if !interesting {
            continue;
        }

        let rel = path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        let bytes = fs::read(path)
            .with_context(|| format!("failed to read {} for fingerprint", path.display()))?;
        let mut h = Sha256::new();
        h.update(&bytes);
        let mut digest = [0u8; 32];
        digest.copy_from_slice(&h.finalize());
        entries.push((rel, digest));
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut h = Sha256::new();
    for (rel, digest) in &entries {
        h.update(rel.as_bytes());
        h.update(&[0]);
        h.update(digest);
        h.update(&[0]);
    }
    let final_digest = h.finalize();
    let mut hex = String::with_capacity(64);
    for byte in final_digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    Ok(hex)
}

pub fn graph_id_for(workspace_hash: &str, fingerprint: &str) -> String {
    let mut h = Sha256::new();
    h.update(workspace_hash.as_bytes());
    h.update(&[0]);
    h.update(fingerprint.as_bytes());
    h.update(&[0]);
    h.update(SCHEMA_VERSION.to_le_bytes());
    let digest = h.finalize();
    let mut hex = String::with_capacity(32);
    for byte in &digest[..16] {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

#[derive(Clone)]
pub struct GraphDatabases {
    pub meta_by_key: Database<Str, Bytes>,
    pub nodes_by_id: Database<Bytes, SerdeBincode<Node>>,
    pub bindings_by_id: Database<Bytes, SerdeBincode<Binding>>,
    pub bindings_by_from_module: Database<Bytes, Bytes>, // NodeId → BindingId, DUP_SORT
    pub bindings_by_target: Database<Bytes, Bytes>,      // NodeId → BindingId, DUP_SORT
    pub children_by_parent: Database<Bytes, Bytes>,      // NodeId → NodeId, DUP_SORT
    pub usages_by_id: Database<Bytes, SerdeBincode<Usage>>,
    pub usages_by_target: Database<Bytes, Bytes>,        // NodeId → UsageId, DUP_SORT
    pub usages_by_consumer: Database<Bytes, Bytes>,      // NodeId → UsageId, DUP_SORT
}

impl GraphDatabases {
    pub fn create(env: &Env<WithoutTls>, wtxn: &mut RwTxn<'_>) -> Result<Self> {
        Ok(Self {
            meta_by_key: open_or_create_str_bytes(env, wtxn, "meta_by_key", false)?,
            nodes_by_id: open_or_create_bytes_bincode(env, wtxn, "nodes_by_id", false)?,
            bindings_by_id: open_or_create_bytes_bincode(env, wtxn, "bindings_by_id", false)?,
            bindings_by_from_module: open_or_create_bytes_bytes(
                env,
                wtxn,
                "bindings_by_from_module",
                true,
            )?,
            bindings_by_target: open_or_create_bytes_bytes(
                env,
                wtxn,
                "bindings_by_target",
                true,
            )?,
            children_by_parent: open_or_create_bytes_bytes(
                env,
                wtxn,
                "children_by_parent",
                true,
            )?,
            usages_by_id: open_or_create_bytes_bincode(env, wtxn, "usages_by_id", false)?,
            usages_by_target: open_or_create_bytes_bytes(env, wtxn, "usages_by_target", true)?,
            usages_by_consumer: open_or_create_bytes_bytes(
                env,
                wtxn,
                "usages_by_consumer",
                true,
            )?,
        })
    }

    pub fn open(env: &Env<WithoutTls>, rtxn: &RoTxn<'_>) -> Result<Option<Self>> {
        let Some(meta_by_key) = env.open_database::<Str, Bytes>(rtxn, Some("meta_by_key"))? else {
            return Ok(None);
        };
        Ok(Some(Self {
            meta_by_key,
            nodes_by_id: env
                .open_database(rtxn, Some("nodes_by_id"))?
                .context("nodes_by_id missing")?,
            bindings_by_id: env
                .open_database(rtxn, Some("bindings_by_id"))?
                .context("bindings_by_id missing")?,
            bindings_by_from_module: env
                .open_database(rtxn, Some("bindings_by_from_module"))?
                .context("bindings_by_from_module missing")?,
            bindings_by_target: env
                .open_database(rtxn, Some("bindings_by_target"))?
                .context("bindings_by_target missing")?,
            children_by_parent: env
                .open_database(rtxn, Some("children_by_parent"))?
                .context("children_by_parent missing")?,
            usages_by_id: env
                .open_database(rtxn, Some("usages_by_id"))?
                .context("usages_by_id missing")?,
            usages_by_target: env
                .open_database(rtxn, Some("usages_by_target"))?
                .context("usages_by_target missing")?,
            usages_by_consumer: env
                .open_database(rtxn, Some("usages_by_consumer"))?
                .context("usages_by_consumer missing")?,
        }))
    }
}

fn open_or_create_str_bytes(
    env: &Env<WithoutTls>,
    wtxn: &mut RwTxn<'_>,
    name: &'static str,
    dup_sort: bool,
) -> Result<Database<Str, Bytes>> {
    let mut opts = env.database_options().types::<Str, Bytes>();
    opts.name(name);
    if dup_sort {
        opts.flags(heed::DatabaseFlags::DUP_SORT);
    }
    Ok(opts.create(wtxn)?)
}

fn open_or_create_bytes_bincode<T>(
    env: &Env<WithoutTls>,
    wtxn: &mut RwTxn<'_>,
    name: &'static str,
    dup_sort: bool,
) -> Result<Database<Bytes, SerdeBincode<T>>>
where
    T: Serialize + serde::de::DeserializeOwned + 'static,
{
    let mut opts = env.database_options().types::<Bytes, SerdeBincode<T>>();
    opts.name(name);
    if dup_sort {
        opts.flags(heed::DatabaseFlags::DUP_SORT);
    }
    Ok(opts.create(wtxn)?)
}

fn open_or_create_bytes_bytes(
    env: &Env<WithoutTls>,
    wtxn: &mut RwTxn<'_>,
    name: &'static str,
    dup_sort: bool,
) -> Result<Database<Bytes, Bytes>> {
    let mut opts = env.database_options().types::<Bytes, Bytes>();
    opts.name(name);
    if dup_sort {
        opts.flags(heed::DatabaseFlags::DUP_SORT);
    }
    Ok(opts.create(wtxn)?)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphManifest {
    pub graph_id: String,
    pub workspace_root: String,
    pub workspace_hash: String,
    pub fingerprint: String,
    pub schema_version: u32,
    pub created_at_unix: u64,
    pub node_count: u64,
    pub binding_count: u64,
    #[serde(default)]
    pub usage_count: u64,
}

pub fn write_manifest(path: &Path, manifest: &GraphManifest) -> Result<()> {
    let json = serde_json::to_string_pretty(manifest)?;
    fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))
}

pub fn read_manifest(path: &Path) -> Result<GraphManifest> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let manifest: GraphManifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse manifest {}", path.display()))?;
    if manifest.schema_version != SCHEMA_VERSION {
        bail!(
            "manifest schema_version {} does not match current {}",
            manifest.schema_version,
            SCHEMA_VERSION
        );
    }
    Ok(manifest)
}

#[allow(dead_code)]
fn _binding_id_marker(_: BindingId) {}
