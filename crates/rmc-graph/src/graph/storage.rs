//! Storage layout, heed environment, and database schema for graph snapshots.
//!
//! Filesystem layout (lifted from the prior attempt — this part worked):
//!
//! ```text
//! <data_dir>/graphs/<workspace_hash>/
//!   CURRENT                ← text file with active graph_id (hex)
//!   BUILD.lock             ← advisory lock: one builder at a time stages,
//!                            publishes and prunes (see snapshot.rs)
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

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use heed::types::{Bytes, SerdeBincode, Str};
use heed::{Database, Env, EnvOpenOptions, RoTxn, RwTxn, WithoutTls};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use super::ids::BindingId;
use super::model::{Binding, EmbeddingRecord, FunctionSignature, Node, StaticMetadata, Usage, UnresolvedCall};

// v2 (2026-05): added usages_by_id / usages_by_target / usages_by_consumer
// sub-databases and `usage_count` to the manifest.
// v3 (2026-05): extraction now populates `Node.file` / `Node.span` for local
// Items via `Definition::try_to_nav`, making `dead_pub_report` findings
// navigable. Schema layout is unchanged — only the extracted data is denser —
// but bumping invalidates v2 snapshots so users see the new fields without
// remembering to `--force`. v1/v2/v3 graph_ids are disjoint (graph_id_for
// hashes SCHEMA_VERSION), so old snapshots stop being reused. (They used to
// remain on disk until a manual cleanup; since 2026-09 every publish removes
// the superseded ones — `snapshot::prune_superseded_snapshots`.)
// v4 (2026-05): Binding gains `is_explicit_pub_use` — true iff the source
// `use` statement carries an explicit `pub`/`pub(crate)`/`pub(in path)`
// visibility token. Backs the `declared_reexports_of` query. Bincode reads
// of v3 records would reject the new field as unexpected EOF, so the bump
// is required even though `#[serde(default)]` would otherwise tolerate
// missing fields under serde_json. v3/v4 graph_ids are disjoint (graph_id_for
// hashes SCHEMA_VERSION).
// v5 (2026-05): Layer 4 — methods, associated consts/types, and trait
// declaration items are emitted as Item nodes with `parent_id` set to the
// host type / trait Item. Adds `ItemKind::Method` (shared by inherent-impl
// fns and trait-declaration fns); `AssocConst` and `AssocType` now also
// surface via the new `extract_impl_items` pass. Old snapshots auto-rebuild
// because `graph_id_for` mixes `SCHEMA_VERSION`.
// v6 (2026-05): Layer 10 — call graph. `Usage` gains
// `consumer_function: Option<NodeId>`, attributing each non-import
// reference to the enclosing function body when one exists (None for
// const initializers / type alias bounds / enum variant discriminants;
// closures attribute to the parent fn per RA's
// `SemanticsScope::containing_function`). Adds a new
// `usages_by_consumer_function` DUP_SORT sub-DB (NodeId → UsageId) that
// powers the `calls_from` query. Bincode reads of v5 records reject the
// extra field as unexpected EOF, so the bump is required even with
// `#[serde(default)]`. v5/v6 graph_ids are disjoint via `graph_id_for`
// hashing `SCHEMA_VERSION`.
// v7 (2026-05): enum variants are now extracted as Item nodes parented to
// their enum's Item (so `who_uses(MyEnum::SomeVariant)` works and
// `enum_variants(enum_id)` can enumerate them). Adds `ItemKind::EnumVariant`.
// No new sub-DB; variants flow through the existing `nodes_by_id` /
// `children_by_parent` / `bindings_by_*` machinery. Schema layout is
// unchanged but the v6 enum-bearing serialized records are missing the new
// variant kind, so the bump is precautionary; old snapshots auto-rebuild
// because `graph_id_for` hashes `SCHEMA_VERSION`.
// v8 (2026-05): Layer 4 — item attributes (derives, doc comments,
// `#[must_use]`, etc.). `Node` gains `attributes: Vec<String>` populated by
// the new `extract_attributes` pass which walks each local Item's AST source
// via `HasSource::source(db)` and collects outer attrs (`#[...]`) and doc
// comments (`/// ...` / `//! ...`). Each attribute is stored as one trimmed
// source-text entry; multi-line doc comments produce one entry per line so
// substring queries match a single line. Inner attributes on items aren't
// collected (they apply to the enclosing module). The new field is
// `#[serde(default)]` for forward compat with older serialized records, but
// existing snapshots still need to rebuild because `graph_id_for` hashes
// `SCHEMA_VERSION`. Backs the new `item_attributes` and
// `items_with_attribute` queries.
// v9 (2026-05): Phase 5 — per-function signature extraction. New
// `extract_signatures` pass walks every local function (free fn, inherent
// assoc fn, trait declaration fn — NOT impl-trait body fns; mirrors the
// impls.rs::69 exclusion) and emits a `FunctionSignature` carrying:
//   * `is_async` flag
//   * self kind (Owned / Ref / RefMut, or None for free fns)
//   * non-self params with name, stringified type, by_ref, mutability
//   * return type as a HirDisplay string
//   * generic type parameters with their declaration-site trait bounds
// Type strings come from `HirDisplay::display(db, dt)` with the function's
// owning crate as `DisplayTarget`; anonymous lifetimes (`'_`) are
// suppressed by default. Adds a new `signatures_by_target` sub-DB
// (NodeId → FunctionSignature) — NOT DUP_SORT, one signature per fn.
// Backs the new `function_signature` and `functions_with_filter` queries.
// `GraphManifest` is unchanged (no `signature_count` field). Old snapshots
// auto-rebuild because `graph_id_for` hashes `SCHEMA_VERSION`.
// v10 (2026-05): Phase 7 Path B — type-aware static-item metadata. New
// `extract_statics` pass walks every local `static` item (ModuleDefId::StaticId
// in `def_to_node`) and emits a `StaticMetadata` record carrying:
//   * `type_string`: the static's declared type rendered via `HirDisplay`
//     against the static's owning crate as `DisplayTarget` (anonymous
//     lifetimes suppressed).
//   * `is_mut`: `true` iff the source uses `static mut FOO` (carries
//     `StaticFlags::MUTABLE`).
// Adds a new `static_metadata_by_target` sub-DB (NodeId → StaticMetadata) —
// NOT DUP_SORT, one record per `static`. Backs the new `static_metadata`
// (single-target lookup) and `mut_static_audit` (workspace-wide pattern
// classifier) queries. `GraphManifest` is unchanged. Old snapshots
// auto-rebuild because `graph_id_for` hashes `SCHEMA_VERSION`.
// v11 (2026-05): Adds a new `embeddings_by_target` sub-DB
// (NodeId → EmbeddingRecord) — one entry per Item whose source has been
// embedded for `semantic_overlaps`. Lazy-populated; `build_hypergraph`
// leaves it empty. Cache key is `(NodeId, content_hash, embedder_version)`:
// content_hash is `SHA-256(source_bytes)` truncated to 16 bytes, and
// `embedder_version` pins the embedding-model identity (so swapping models
// auto-invalidates entries). Old v10 snapshots auto-rebuild because
// `graph_id_for` hashes `SCHEMA_VERSION`.
// v12 (2026-05): `Node` gains `crate_target_kind` for crate nodes, populated
// from Cargo target metadata (`lib`, `bin`, `example`, `test`, `bench`,
// `build`, or `unknown`). This lets `forbidden_dependency_check` default to
// architecture checks over only lib/bin consumers while excluding examples,
// tests, benches, and build scripts unless callers opt them in. Existing v11
// Node records are missing the appended bincode field, so old snapshots
// auto-rebuild via the schema-versioned graph id.
// v13 (2026-10): every row has one owning crate, so a crate's rows can be
// deleted and rewritten without touching anyone else's (see `ownership.rs`
// for the rule). Two new DUP_SORT sub-DBs: `owned_rows_by_crate` (crate
// NodeId → encoded `(DbTag, key, dup value)` of each row it owns) and
// `stub_owners` (ExternalSymbol NodeId → crate NodeId, one per crate whose
// bindings name the stub). No existing table changes; v12 snapshots
// auto-rebuild because `graph_id_for` hashes `SCHEMA_VERSION`.
// v14 (2026-10): an ExternalSymbol stub for a module is named by the module's
// own path (`alloc::fmt`), not `<path>::<name of the first import met>`
// (`alloc::fmt::f`), so its NodeId no longer depends on which crate's walk
// met it first — the per-crate build split one module into a stub per crate.
// No table changes, but a v13 snapshot's module stubs carry the old ids and an
// incremental rebuild over it would mix both namings: the bump forces one
// full rebuild.
// v15: usages carry semantic relations and structural owners; impl bodies,
// fields and block-local items are nodes. Existing v14 data must be rebuilt,
// not relabelled as calls from target kinds. Row ownership remains per crate.
// Adds usages_by_consumer_item and unresolved_calls_by_owner, both recorded
// in owned_rows_by_crate and covered by snapshot comparison/deletion.
// v16: Cargo-target identities, offset-independent associated/variant/local
// item ids, explicit declaration modules, macro expansion provenance and
// coherent immutable build instances. v15 snapshots can contain mixed source
// versions and must never be reused as v16 inputs.
pub(crate) const SCHEMA_VERSION: u32 = 16;
pub(crate) const CURRENT_POINTER_FILENAME: &str = "CURRENT";
pub(crate) const BUILD_LOCK_FILENAME: &str = "BUILD.lock";
pub(crate) const SNAPSHOTS_DIRNAME: &str = "snapshots";
pub(crate) const MANIFEST_FILENAME: &str = "manifest.json";

// A map is an address-space reservation, not an eager RAM/disk allocation.
// The complete graph of a large workspace exceeds 1 GiB with its canonical rows and
// secondary indexes. Keep headroom on 64-bit hosts; narrower hosts retain the
// original limit. Callers can still choose their own GraphEnvOptions.map_size.
#[cfg(target_pointer_width = "64")]
const DEFAULT_MAP_SIZE: usize = 16 << 30; // 16 GiB
#[cfg(not(target_pointer_width = "64"))]
const DEFAULT_MAP_SIZE: usize = 1 << 30; // 1 GiB
// v13 uses 15 named sub-DBs; the margin keeps the next table from failing
// with MDB_DBS_FULL on open.
const DEFAULT_MAX_DBS: u32 = 32;
const DEFAULT_MAX_READERS: u32 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    /// Lives beside `CURRENT`, not inside `snapshots/`, so that the prune —
    /// which walks `snapshots/` — never sees it as a snapshot.
    pub fn build_lock_path(&self) -> PathBuf {
        self.root_dir.join(BUILD_LOCK_FILENAME)
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

pub(in crate::graph) fn is_source_walk_excluded(path: &Path) -> bool {
    path.components().any(|c| {
        c.as_os_str() == "target"
            || c.as_os_str() == ".git"
            || c.as_os_str() == ".skeleton"
    })
}

/// The tree's fingerprint — what decides whether a snapshot is still current
/// (and, through [`graph_id_for`], names it). Every input of
/// [`compute_build_inputs`]. The build takes it from there; tests want it alone.
#[cfg(test)]
pub(crate) fn compute_fingerprint(workspace_root: &Path) -> Result<String> {
    Ok(compute_build_inputs(workspace_root)?.fingerprint())
}

/// What a build reads from the tree, split the way an incremental build
/// invalidates it (plan step `package-fingerprint`).
///
/// A *package* here is a directory holding a `Cargo.toml` — a workspace
/// member, a vendored patch crate, a nested workspace the build never loads —
/// plus the workspace root itself (key `""`), which exists even when the root
/// has no manifest. Every fingerprinted file belongs to the deepest package
/// directory above it.
///
/// Fingerprinted are `.rs` files, every `Cargo.toml` and `Cargo.lock`, and
/// at the root `rust-toolchain.toml` / `rust-toolchain` and
/// `.cargo/config.toml` / `.cargo/config`; `target/`, `.git/` and
/// `.skeleton/` are not walked ([`is_source_walk_excluded`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildInputs {
    /// Hash of what makes any edit a full rebuild: the root `Cargo.lock`, the
    /// toolchain and cargo config files, and every `Cargo.toml` with its path
    /// — so the set of packages too.
    pub global: String,
    /// Package directory, workspace-relative with `/` separators (`""` for
    /// the root) → hash of the files that belong to it, `Cargo.toml` included.
    pub packages: BTreeMap<String, String>,
}

impl BuildInputs {
    /// One hash over [`Self::global`] and every package's.
    pub fn fingerprint(&self) -> String {
        let mut h = Sha256::new();
        h.update(b"global\0");
        h.update(self.global.as_bytes());
        h.update(&[0]);
        for (dir, hash) in &self.packages {
            h.update(dir.as_bytes());
            h.update(&[0]);
            h.update(hash.as_bytes());
            h.update(&[0]);
        }
        hex(&h.finalize())
    }

    /// The package `rel_path` (workspace-relative, `/` separators) belongs
    /// to: the deepest package directory above it, the root at worst.
    pub fn package_of<'a>(&'a self, rel_path: &str) -> &'a str {
        let mut dir = rel_path;
        while let Some(cut) = dir.rfind('/') {
            dir = &dir[..cut];
            if let Some((key, _)) = self.packages.get_key_value(dir) {
                return key;
            }
        }
        ""
    }

    /// Packages whose hash differs between `self` and `other`, or that only
    /// one of them has.
    pub fn changed_packages(&self, other: &BuildInputs) -> BTreeSet<String> {
        let mut changed = BTreeSet::new();
        for (dir, hash) in &self.packages {
            if other.packages.get(dir) != Some(hash) {
                changed.insert(dir.clone());
            }
        }
        for dir in other.packages.keys() {
            if !self.packages.contains_key(dir) {
                changed.insert(dir.clone());
            }
        }
        changed
    }
}

/// Walk the tree once and hash it into [`BuildInputs`].
pub(crate) fn compute_build_inputs(workspace_root: &Path) -> Result<BuildInputs> {
    // (workspace-relative path with `/`, sha of the bytes), sorted by path.
    let mut files: Vec<(String, [u8; 32])> = Vec::new();

    for entry in WalkDir::new(workspace_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !is_source_walk_excluded(e.path()))
    {
        let entry = entry.with_context(|| format!("walk graph inputs at {}", workspace_root.display()))?;
        let path = entry.path();
        if !entry.file_type().is_file() && !(entry.file_type().is_symlink() && fs::metadata(path)?.is_file()) {
            continue;
        }
        let rel = path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if !is_fingerprinted(&rel) {
            continue;
        }
        let bytes = fs::read(path)
            .with_context(|| format!("failed to read {} for fingerprint", path.display()))?;
        let mut digest = [0u8; 32];
        digest.copy_from_slice(&Sha256::digest(&bytes));
        files.push((rel, digest));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut packages: BTreeMap<String, String> = BTreeMap::new();
    packages.insert(String::new(), String::new());
    for (rel, _) in &files {
        if let Some(dir) = rel.strip_suffix("Cargo.toml") {
            // `"Cargo.toml"` → `""`, `"a/b/Cargo.toml"` → `"a/b"`.
            if dir.is_empty() || dir.ends_with('/') {
                packages.insert(dir.trim_end_matches('/').to_string(), String::new());
            }
        }
    }
    let shape = BuildInputs {
        global: String::new(),
        packages,
    };

    let mut global = Sha256::new();
    let mut per_package: BTreeMap<&str, Sha256> = BTreeMap::new();
    for (rel, digest) in &files {
        let is_global = rel.ends_with("Cargo.toml")
            || matches!(
                rel.as_str(),
                "Cargo.lock"
                    | "rust-toolchain.toml"
                    | "rust-toolchain"
                    | ".cargo/config.toml"
                    | ".cargo/config"
            );
        if is_global {
            global.update(rel.as_bytes());
            global.update(&[0]);
            global.update(digest);
            global.update(&[0]);
        }
        let h = per_package.entry(shape.package_of(rel)).or_default();
        h.update(rel.as_bytes());
        h.update(&[0]);
        h.update(digest);
        h.update(&[0]);
    }
    let packages = shape
        .packages
        .keys()
        .map(|dir| {
            let hash = per_package
                .remove(dir.as_str())
                .unwrap_or_default()
                .finalize();
            (dir.clone(), hex(&hash))
        })
        .collect();
    Ok(BuildInputs {
        global: hex(&global.finalize()),
        packages,
    })
}

/// Whether a workspace-relative path is one of the files
/// [`compute_build_inputs`] hashes.
pub(super) fn is_fingerprinted(rel: &str) -> bool {
    let file_name = rel.rsplit('/').next().unwrap_or(rel);
    rel.ends_with(".rs")
        || matches!(file_name, "Cargo.toml" | "Cargo.lock")
        || matches!(
            rel,
            "rust-toolchain.toml" | "rust-toolchain" | ".cargo/config.toml" | ".cargo/config"
        )
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(2 * bytes.len());
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub(crate) fn graph_id_for(workspace_hash: &str, fingerprint: &str) -> String {
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
    pub unresolved_calls_by_owner: Database<Bytes, SerdeBincode<Vec<UnresolvedCall>>>,
    pub usages_by_consumer_item: Database<Bytes, Bytes>,
    pub meta_by_key: Database<Str, Bytes>,
    pub nodes_by_id: Database<Bytes, SerdeBincode<Node>>,
    pub bindings_by_id: Database<Bytes, SerdeBincode<Binding>>,
    pub bindings_by_from_module: Database<Bytes, Bytes>, // NodeId → BindingId, DUP_SORT
    pub bindings_by_target: Database<Bytes, Bytes>,      // NodeId → BindingId, DUP_SORT
    pub children_by_parent: Database<Bytes, Bytes>,      // NodeId → NodeId, DUP_SORT
    pub usages_by_id: Database<Bytes, SerdeBincode<Usage>>,
    pub usages_by_target: Database<Bytes, Bytes>,        // NodeId → UsageId, DUP_SORT
    pub usages_by_consumer: Database<Bytes, Bytes>,      // NodeId → UsageId, DUP_SORT
    pub usages_by_consumer_function: Database<Bytes, Bytes>, // NodeId → UsageId, DUP_SORT
    /// v9: NodeId (target fn) → FunctionSignature. NOT DUP_SORT — one
    /// signature per local function.
    pub signatures_by_target: Database<Bytes, SerdeBincode<FunctionSignature>>,
    /// v10: NodeId (target static) → StaticMetadata. NOT DUP_SORT — one
    /// record per local `static` item.
    pub static_metadata_by_target: Database<Bytes, SerdeBincode<StaticMetadata>>,
    /// v11: lazy-populated cache for `semantic_overlaps`. NodeId →
    /// EmbeddingRecord. NOT DUP_SORT — one record per Item. Empty after
    /// `build_hypergraph`; `semantic_overlaps` writes entries on first use
    /// and reuses them on subsequent scans of unchanged items.
    pub embeddings_by_target: Database<Bytes, SerdeBincode<EmbeddingRecord>>,
    /// v13: ExternalSymbol NodeId → crate NodeId, DUP_SORT — every crate
    /// whose bindings name the stub. The stub node goes when its last owner
    /// does.
    pub stub_owners: Database<Bytes, Bytes>,
    /// v13: crate NodeId → `OwnedRow` encoding, DUP_SORT — the exact list of
    /// rows to delete when that crate is rebuilt.
    pub owned_rows_by_crate: Database<Bytes, Bytes>,
}

impl GraphDatabases {
    pub fn create(env: &Env<WithoutTls>, wtxn: &mut RwTxn<'_>) -> Result<Self> {
        Ok(Self {
            unresolved_calls_by_owner: open_or_create_bytes_bincode(env, wtxn, "unresolved_calls_by_owner", false)?,
            usages_by_consumer_item: open_or_create_bytes_bytes(env, wtxn, "usages_by_consumer_item", true)?,
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
            usages_by_consumer_function: open_or_create_bytes_bytes(
                env,
                wtxn,
                "usages_by_consumer_function",
                true,
            )?,
            signatures_by_target: open_or_create_bytes_bincode(
                env,
                wtxn,
                "signatures_by_target",
                false,
            )?,
            static_metadata_by_target: open_or_create_bytes_bincode(
                env,
                wtxn,
                "static_metadata_by_target",
                false,
            )?,
            embeddings_by_target: open_or_create_bytes_bincode(
                env,
                wtxn,
                "embeddings_by_target",
                false,
            )?,
            stub_owners: open_or_create_bytes_bytes(env, wtxn, "stub_owners", true)?,
            owned_rows_by_crate: open_or_create_bytes_bytes(
                env,
                wtxn,
                "owned_rows_by_crate",
                true,
            )?,
        })
    }

    pub fn open(env: &Env<WithoutTls>, rtxn: &RoTxn<'_>) -> Result<Option<Self>> {
        let Some(meta_by_key) = env.open_database::<Str, Bytes>(rtxn, Some("meta_by_key"))? else {
            return Ok(None);
        };
        Ok(Some(Self {
            unresolved_calls_by_owner: env.open_database(rtxn, Some("unresolved_calls_by_owner"))?.context("unresolved_calls_by_owner missing")?,
            usages_by_consumer_item: env.open_database(rtxn, Some("usages_by_consumer_item"))?.context("usages_by_consumer_item missing")?,
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
            usages_by_consumer_function: env
                .open_database(rtxn, Some("usages_by_consumer_function"))?
                .context("usages_by_consumer_function missing")?,
            signatures_by_target: env
                .open_database(rtxn, Some("signatures_by_target"))?
                .context("signatures_by_target missing")?,
            static_metadata_by_target: env
                .open_database(rtxn, Some("static_metadata_by_target"))?
                .context("static_metadata_by_target missing")?,
            embeddings_by_target: env
                .open_database(rtxn, Some("embeddings_by_target"))?
                .context("embeddings_by_target missing")?,
            stub_owners: env
                .open_database(rtxn, Some("stub_owners"))?
                .context("stub_owners missing")?,
            owned_rows_by_crate: env
                .open_database(rtxn, Some("owned_rows_by_crate"))?
                .context("owned_rows_by_crate missing")?,
        }))
    }
}

/// One sub-DB of a snapshot that a build writes — every one except
/// `meta_by_key` (fingerprint, graph id, counts) and `embeddings_by_target`
/// (a query-time cache).
///
/// The declaration order is the order the snapshot oracle reports tables in.
/// The discriminant is the table's code inside an `owned_rows_by_crate`
/// value, so it is on disk: never renumber, only append.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum DbTag {
    NodesById = 1,
    BindingsById = 2,
    BindingsByFromModule = 3,
    BindingsByTarget = 4,
    ChildrenByParent = 5,
    UsagesById = 6,
    UsagesByTarget = 7,
    UsagesByConsumer = 8,
    UsagesByConsumerFunction = 9,
    SignaturesByTarget = 10,
    StaticMetadataByTarget = 11,
    StubOwners = 12,
    OwnedRowsByCrate = 13,
    UnresolvedCallsByOwner = 14,
    UsagesByConsumerItem = 15,
}

impl DbTag {
    pub const ALL: [DbTag; 15] = [
        DbTag::NodesById,
        DbTag::BindingsById,
        DbTag::BindingsByFromModule,
        DbTag::BindingsByTarget,
        DbTag::ChildrenByParent,
        DbTag::UsagesById,
        DbTag::UsagesByTarget,
        DbTag::UsagesByConsumer,
        DbTag::UsagesByConsumerFunction,
        DbTag::SignaturesByTarget,
        DbTag::StaticMetadataByTarget,
        DbTag::StubOwners,
        DbTag::OwnedRowsByCrate,
        DbTag::UnresolvedCallsByOwner,
        DbTag::UsagesByConsumerItem,
    ];

    /// The LMDB name of the sub-DB.
    pub fn db_name(self) -> &'static str {
        match self {
            DbTag::NodesById => "nodes_by_id",
            DbTag::BindingsById => "bindings_by_id",
            DbTag::BindingsByFromModule => "bindings_by_from_module",
            DbTag::BindingsByTarget => "bindings_by_target",
            DbTag::ChildrenByParent => "children_by_parent",
            DbTag::UsagesById => "usages_by_id",
            DbTag::UsagesByTarget => "usages_by_target",
            DbTag::UsagesByConsumer => "usages_by_consumer",
            DbTag::UsagesByConsumerFunction => "usages_by_consumer_function",
            DbTag::SignaturesByTarget => "signatures_by_target",
            DbTag::StaticMetadataByTarget => "static_metadata_by_target",
            DbTag::StubOwners => "stub_owners",
            DbTag::OwnedRowsByCrate => "owned_rows_by_crate",
            DbTag::UnresolvedCallsByOwner => "unresolved_calls_by_owner",
            DbTag::UsagesByConsumerItem => "usages_by_consumer_item",
        }
    }

    /// Whether one key holds many values (LMDB DUP_SORT). Deleting a row of
    /// such a table needs the value too, not only the key.
    pub fn is_dup_sort(self) -> bool {
        match self {
            DbTag::NodesById
            | DbTag::BindingsById
            | DbTag::UsagesById
            | DbTag::SignaturesByTarget
            | DbTag::StaticMetadataByTarget
            | DbTag::UnresolvedCallsByOwner => false,
            DbTag::BindingsByFromModule
            | DbTag::BindingsByTarget
            | DbTag::ChildrenByParent
            | DbTag::UsagesByTarget
            | DbTag::UsagesByConsumer
            | DbTag::UsagesByConsumerFunction
            | DbTag::StubOwners
            | DbTag::OwnedRowsByCrate
            | DbTag::UsagesByConsumerItem => true,
        }
    }

    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<DbTag> {
        DbTag::ALL.into_iter().find(|t| t.code() == code)
    }

    /// The table, with its key and value types erased to raw bytes.
    pub fn raw(self, dbs: &GraphDatabases) -> Database<Bytes, Bytes> {
        match self {
            DbTag::NodesById => dbs.nodes_by_id.remap_types(),
            DbTag::BindingsById => dbs.bindings_by_id.remap_types(),
            DbTag::BindingsByFromModule => dbs.bindings_by_from_module,
            DbTag::BindingsByTarget => dbs.bindings_by_target,
            DbTag::ChildrenByParent => dbs.children_by_parent,
            DbTag::UsagesById => dbs.usages_by_id.remap_types(),
            DbTag::UsagesByTarget => dbs.usages_by_target,
            DbTag::UsagesByConsumer => dbs.usages_by_consumer,
            DbTag::UsagesByConsumerFunction => dbs.usages_by_consumer_function,
            DbTag::SignaturesByTarget => dbs.signatures_by_target.remap_types(),
            DbTag::StaticMetadataByTarget => dbs.static_metadata_by_target.remap_types(),
            DbTag::StubOwners => dbs.stub_owners,
            DbTag::OwnedRowsByCrate => dbs.owned_rows_by_crate,
            DbTag::UnresolvedCallsByOwner => dbs.unresolved_calls_by_owner.remap_types(),
            DbTag::UsagesByConsumerItem => dbs.usages_by_consumer_item,
        }
    }
}

impl std::fmt::Display for DbTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.db_name())
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
    /// Captured by the builder, never reconstructed from the exporting process.
    /// Absent in older v16 JSON manifests; absence means unknown provenance.
    #[serde(default)]
    pub extraction_context: Option<serde_json::Value>,
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
    #[serde(default)]
    pub usage_counts: Option<super::model::UsageCounts>,
    /// What the snapshot was built from, by package — the base an
    /// incremental build diffs today's tree against. `None` in manifests
    /// written before incremental builds: the next build is a full one.
    #[serde(default)]
    pub inputs: Option<BuildInputs>,
    /// Crate (group) name → the packages its module files lie in
    /// ([`BuildInputs::package_of`]). Usually just its own; more when a
    /// `#[path]` reaches into another package. An incremental build rebuilds a
    /// crate when one of these packages changed.
    #[serde(default)]
    pub crate_packages: BTreeMap<String, BTreeSet<String>>,
}

pub(crate) fn write_manifest(path: &Path, manifest: &GraphManifest) -> Result<()> {
    let json = serde_json::to_string_pretty(manifest)?;
    fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))
}

pub(crate) fn read_manifest(path: &Path) -> Result<GraphManifest> {
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

/// Soft-fail variant of [`read_manifest`]. Returns `Ok(None)` when the
/// manifest is well-formed but encodes a different `schema_version` than
/// the running server. Real errors (missing file, malformed JSON) still
/// propagate.
///
/// Use this from read paths where a stale snapshot should be reported as
/// "no compatible snapshot available — call build_hypergraph first" rather
/// than as an opaque internal error.
pub(crate) fn read_manifest_compatible(path: &Path) -> Result<Option<GraphManifest>> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let manifest: GraphManifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse manifest {}", path.display()))?;
    if manifest.schema_version != SCHEMA_VERSION {
        tracing::warn!(
            stored_schema_version = manifest.schema_version,
            current_schema_version = SCHEMA_VERSION,
            manifest_path = %path.display(),
            "ignoring incompatible snapshot manifest (schema_version mismatch)"
        );
        return Ok(None);
    }
    Ok(Some(manifest))
}

#[allow(dead_code)]
fn _binding_id_marker(_: BindingId) {}

#[cfg(test)]
mod tests {
    //! Fingerprint sanity. These cover what `compute_fingerprint` actually
    //! observes — important for trusting `force_rebuild` semantics, since
    //! the rebuild path keys off `graph_id_for(workspace_hash, fingerprint)`
    //! and a stale fingerprint would let the call return early via the
    //! "manifest_path exists" branch in `build_and_persist`.

    use super::*;

    fn write_minimal_crate(dir: &Path) {
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"fp_test\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/lib.rs"), "pub fn answer() -> i32 { 42 }\n").unwrap();
    }

    #[test]
    fn fingerprint_changes_when_rs_file_edited() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        write_minimal_crate(root);
        let before = compute_fingerprint(root).unwrap();

        // Smallest possible byte-level edit: append a trailing newline.
        fs::write(root.join("src/lib.rs"), "pub fn answer() -> i32 { 42 }\n\n").unwrap();
        let after = compute_fingerprint(root).unwrap();

        assert_ne!(
            before, after,
            "fingerprint must flip on .rs byte change; force_rebuild relies on this"
        );
    }

    #[test]
    fn fingerprint_changes_when_cargo_toml_edited() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        write_minimal_crate(root);
        let before = compute_fingerprint(root).unwrap();

        // Bump version — should flip fingerprint.
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"fp_test\"\nversion = \"0.0.1\"\nedition = \"2021\"\n",
        )
        .unwrap();
        let after = compute_fingerprint(root).unwrap();

        assert_ne!(
            before, after,
            "fingerprint must flip on Cargo.toml byte change"
        );
    }

    #[test]
    fn fingerprint_stable_when_target_dir_grows() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        write_minimal_crate(root);
        let before = compute_fingerprint(root).unwrap();

        // Simulate a build artifact under target/ — should be ignored.
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::write(root.join("target/debug/fake.rs"), "fn main() {}\n").unwrap();
        fs::write(root.join("target/.lockfile"), b"x").unwrap();

        let after = compute_fingerprint(root).unwrap();
        assert_eq!(
            before, after,
            "target/ contents must not affect fingerprint"
        );
    }

    #[test]
    fn fingerprint_stable_when_git_dir_grows() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        write_minimal_crate(root);
        let before = compute_fingerprint(root).unwrap();

        fs::create_dir_all(root.join(".git/objects")).unwrap();
        fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(root.join(".git/objects/abcdef"), b"xx").unwrap();

        let after = compute_fingerprint(root).unwrap();
        assert_eq!(
            before, after,
            ".git/ contents must not affect fingerprint"
        );
    }

    #[test]
    fn fingerprint_stable_when_skeleton_dir_grows() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        write_minimal_crate(root);
        let before = compute_fingerprint(root).unwrap();

        fs::create_dir_all(root.join(".skeleton/src")).unwrap();
        fs::write(root.join(".skeleton/src/lib.rs"), "pub fn generated() {}\n").unwrap();
        fs::write(root.join(".skeleton/manifest.json"), "{}\n").unwrap();

        let after = compute_fingerprint(root).unwrap();
        assert_eq!(
            before, after,
            ".skeleton/ contents must not affect fingerprint"
        );
    }

    #[test]
    fn fingerprint_stable_when_unrelated_file_added() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        write_minimal_crate(root);
        let before = compute_fingerprint(root).unwrap();

        // README, JSON, dotfile — none should be fingerprinted.
        fs::write(root.join("README.md"), "# fp_test\n").unwrap();
        fs::write(root.join("data.json"), "{}\n").unwrap();
        fs::write(root.join(".env"), "KEY=value\n").unwrap();

        let after = compute_fingerprint(root).unwrap();
        assert_eq!(
            before, after,
            "non-.rs / non-Cargo.{{toml,lock}} files must not affect fingerprint"
        );
    }

    #[test]
    fn fingerprint_stable_when_only_path_metadata_changes() {
        // Sanity: re-running with no edits returns the same hash. Sentinel for
        // any future drift in WalkDir ordering or filter behavior.
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        write_minimal_crate(root);
        let a = compute_fingerprint(root).unwrap();
        let b = compute_fingerprint(root).unwrap();
        assert_eq!(a, b, "fingerprint must be deterministic across calls");
    }

    /// A file belongs to the deepest package above it; a nested package's
    /// edit changes only that package; any `Cargo.toml` edit is global.
    #[test]
    fn build_inputs_split_the_tree_by_package() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        write_minimal_crate(root);
        fs::create_dir_all(root.join("tools/probe/src")).unwrap();
        fs::write(
            root.join("tools/probe/Cargo.toml"),
            "[package]\nname = \"probe\"\nversion = \"0.0.0\"\n",
        )
        .unwrap();
        fs::write(root.join("tools/probe/src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(root.join("tools/loose.rs"), "fn loose() {}\n").unwrap();

        let before = compute_build_inputs(root).unwrap();
        assert_eq!(
            before.packages.keys().map(String::as_str).collect::<Vec<_>>(),
            ["", "tools/probe"]
        );
        assert_eq!(before.package_of("tools/probe/src/main.rs"), "tools/probe");
        assert_eq!(before.package_of("tools/loose.rs"), "");
        assert_eq!(before.package_of("src/lib.rs"), "");

        fs::write(root.join("tools/probe/src/main.rs"), "fn main() { }\n").unwrap();
        let body_edit = compute_build_inputs(root).unwrap();
        assert_eq!(body_edit.global, before.global);
        assert!(body_edit.changed_packages(&before).iter().eq(["tools/probe"]));
        assert_ne!(body_edit.fingerprint(), before.fingerprint());

        fs::write(
            root.join("tools/probe/Cargo.toml"),
            "[package]\nname = \"probe\"\nversion = \"0.0.1\"\n",
        )
        .unwrap();
        assert_ne!(compute_build_inputs(root).unwrap().global, body_edit.global);
    }

    #[test]
    fn graph_id_changes_with_fingerprint() {
        // Documenting the rebuild contract: graph_id is derived from
        // (workspace_hash, fingerprint, SCHEMA_VERSION). A flipped fingerprint
        // → different graph_id → different snapshot_dir, so force_rebuild and
        // normal rebuild both end up at a fresh location.
        let wh = "ws_abc";
        let a = graph_id_for(wh, "fp_1");
        let b = graph_id_for(wh, "fp_2");
        assert_ne!(a, b, "graph_id must change when fingerprint changes");
    }
}
