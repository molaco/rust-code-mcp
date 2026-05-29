# Section F — P1.3 Analyze / Vision Layer

## Overview

This slice introduces an `analyze` subtree inside `rmc-graph` that turns the per-Item embedding cache (`embeddings_by_target`) and the call/usage substrates into the mesoscale "city map" the navigator (P1.1) renders at the cluster zoom level. It computes per-Item joined feature vectors (Qwen3 embedding ⊕ structural features), clusters them with GMM over the joined substrate (with a spectral fallback over the Laplacian of the call graph), labels each cluster with a 2–5 word concept name produced by the same small LLM that P1.2 uses, scores per-Item outliers (LOF + per-cluster Mahalanobis), and emits two queryable affinity scores (`affinity`: random-walk PMI on the call/usage graph; `co_change`: file-co-occurrence lift from `jj log` / `git log --name-only`). All outputs land in a single `VisionIndex` value cached per `(graph_id, head_commit_hash)`.

M1 work, runs against the slow published snapshot, no P0.2 dependency. Load-bearing for perception (Issue #9): (a) soft membership, not hard assignments; (b) zoom-through to raw nodes via `assignment: HashMap<NodeId, Vec<(ClusterId, f32)>>`; (c) silhouette + Davies–Bouldin per build for quality monitoring; (d) deterministic `seed` from `BuildOptions` threaded through every random source. Fills P1.1's "cluster scale stub" via `clusters_at_zoom(scale)` that the navigator's `MapPane::Cluster` layer reads.

## New modules / files

- `crates/rmc-graph/src/analyze.rs` — crate-entry surface (file-based module; no `mod.rs`) + `build_vision()` entry point + `EmbeddingsLookup`: `ClusterId`, `Cluster`, `OutlierFinding`, `OutlierKind`, `AffinityIndex`, `CoChangeIndex`, `VisionIndex`, `BuildVisionOptions`, `AnalyzeError`, `LabelGenerator`. Declares `mod features; mod cluster; …` for the subtree below.
- `crates/rmc-graph/src/analyze/features.rs` — `FeatureVector`, `StructuralFeatures`, `build_features()`.
- `crates/rmc-graph/src/analyze/cluster.rs` — GMM via `linfa-clustering::GaussianMixtureModel` + spectral fallback (`petgraph` Laplacian + `nalgebra` symmetric eigen + k-means).
- `crates/rmc-graph/src/analyze/outliers.rs` — LOF via `linfa-anomaly::LocalOutlierFactor` + per-cluster Mahalanobis using `nalgebra`.
- `crates/rmc-graph/src/analyze/affinity.rs` — biased random walks on merged `petgraph` graph; pair-count → PMI.
- `crates/rmc-graph/src/analyze/cochange.rs` — async wrapper shelling out to `jj log` (preferred) or `git log --name-only` (fallback).
- `crates/rmc-graph/src/analyze/labels.rs` — LLM-based cluster labeling via P1.2's model handle.
- `crates/rmc-graph/src/analyze/cache.rs` — per-episode cache keyed `(graph_id, head_commit_hash)`. JSON files under `working/<session_id>/vision/<key>.json`.
- `crates/rmc-graph/src/analyze/zoom.rs` — `clusters_at_zoom(scale)` for `MapPane::Cluster`.
- `crates/rmc-graph/src/lib.rs` — `pub mod analyze;`.
- `crates/rmc-graph/Cargo.toml` — the deps below are declared `optional = true` and the new **additive** `analyze` feature enables them via `dep:` syntax (no implicit feature is created):
  ```toml
  [dependencies]
  petgraph        = { version = "0.6",  optional = true }
  linfa           = { version = "0.7",  optional = true }
  linfa-clustering = { version = "0.7", optional = true }
  linfa-anomaly   = { version = "0.7",  optional = true }
  nalgebra        = { version = "0.33", optional = true }
  ndarray         = { version = "0.16", optional = true }
  rand            = { version = "0.8",  optional = true }
  rand_chacha     = { version = "0.8",  optional = true }
  siphasher       = { version = "1",    optional = true }  # stable fixed-key SipHash for seed derivation (§17)

  [features]
  analyze = [
      "dep:petgraph", "dep:linfa", "dep:linfa-clustering", "dep:linfa-anomaly",
      "dep:nalgebra", "dep:ndarray", "dep:rand", "dep:rand_chacha",
      "dep:siphasher",
  ]
  ```

## Type definitions

```rust
// crates/rmc-graph/src/analyze.rs  (crate-entry surface; file-based, no mod.rs)

/// Stable identifier for a cluster within one `VisionIndex`.
/// Private inner field — construct only via `ClusterId::new`.
/// Derived by hashing (graph_id, head_commit_hash, "cluster", local_idx)
/// with the workspace-pinned stable hash (see Determinism plumbing, step 12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClusterId(u32);
impl ClusterId {
    #[must_use] pub fn new(raw: u32) -> Self { Self(raw) }
    #[must_use] pub fn get(self) -> u32 { self.0 }
}

/// 2–5 word concept label for a cluster. Private inner; sanitized on
/// construction (trim, collapse whitespace, cap to 5 words).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterLabel(String);
impl ClusterLabel {
    #[must_use] pub fn as_str(&self) -> &str { &self.0 }
}

/// Identifier of the published graph snapshot this index was built from.
/// Private inner field; mirror of `BuildOptions`' graph id (P0.1).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GraphId(String);
impl GraphId {
    #[must_use] pub fn new(raw: impl Into<String>) -> Self { Self(raw.into()) }
    #[must_use] pub fn as_str(&self) -> &str { &self.0 }
}

/// VCS commit hash (jj change id or git SHA) at index build time.
/// Empty hash disables caching (see step 10). Private inner field.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CommitHash(String);
impl CommitHash {
    #[must_use] pub fn new(raw: impl Into<String>) -> Self { Self(raw.into()) }
    #[must_use] pub fn as_str(&self) -> &str { &self.0 }
    #[must_use] pub fn is_empty(&self) -> bool { self.0.is_empty() }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Cluster {
    pub id: ClusterId,
    pub members: Vec<(NodeId, f32)>,    // (id, soft_membership), sorted desc by total_cmp
    pub centroid: Vec<f32>,
    pub silhouette: f32,                 // finite by construction (see NaN policy)
    pub davies_bouldin_contrib: f32,     // finite by construction
    pub label: Option<ClusterLabel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum OutlierKind { LocalLOF, Mahalanobis, UnclusteredLone }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct OutlierFinding { pub item: NodeId, pub cluster: ClusterId, pub score: f32, pub kind: OutlierKind }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffinityIndex { pairs: HashMap<(NodeId, NodeId), f32> }
impl AffinityIndex { pub fn score(&self, a: NodeId, b: NodeId) -> f32; }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoChangeIndex { pairs: HashMap<(NodeId, NodeId), f32>, pub window: Duration }
impl CoChangeIndex {
    /// Returns `0.0` for unknown / self pairs; all stored lifts are finite and ≥ 0.
    pub fn score(&self, a: NodeId, b: NodeId) -> f32;
    /// Build the co-change lift index by shelling out to `jj log` / `git log`.
    ///
    /// # Errors
    /// - [`AnalyzeError::Vcs`] if the VCS subprocess fails to spawn, exits
    ///   non-zero, or emits output that cannot be parsed.
    /// - [`AnalyzeError::Numeric`] if PMI normalization would divide by zero
    ///   (guarded; see NaN policy).
    pub async fn build_from_vcs(
        snap: &OpenedSnapshot,
        workspace_root: &Path,
        window: Duration,
    ) -> Result<Self, AnalyzeError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct VisionIndex {
    pub graph_id: GraphId,
    pub head_commit_hash: CommitHash,
    pub clusters: Vec<Cluster>,
    pub assignment: HashMap<NodeId, Vec<(ClusterId, f32)>>,
    pub outliers: Vec<OutlierFinding>,
    pub affinity: AffinityIndex,
    pub co_change: CoChangeIndex,
    pub quality: VisionQuality,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct VisionQuality {
    pub silhouette_mean: f32,            // finite; NaN coerced to 0.0 (see NaN policy)
    pub davies_bouldin: f32,             // finite; NaN coerced to f32::INFINITY
    pub picked_k: usize,
    pub bic_curve: Vec<(usize, f64)>,    // BIC values finite; argmin via f64::total_cmp
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct BuildVisionOptions {
    pub seed: u64,
    pub k_candidates: Vec<usize>,          // default vec![8, 16, 32, 64]
    pub min_cluster_weight: f32,           // default 0.05
    pub lof_k: usize,                       // default 20
    pub walks_per_node: usize,              // default 10
    pub walk_length: usize,                 // default 40
    pub node2vec_p: f32,                    // default 1.0
    pub node2vec_q: f32,                    // default 1.0
    pub cochange_window: Duration,          // default 90 days
    pub labeler: Option<Arc<dyn LabelGenerator>>,  // Debug+Send+Sync via supertraits
    pub cache_dir: Option<PathBuf>,
}

impl Default for BuildVisionOptions {
    /// Documented defaults (also listed inline above): a balanced k-sweep,
    /// a 5% soft-membership floor, LOF over 20 neighbors, 10 node2vec walks of
    /// length 40 with unbiased p=q=1.0, a 90-day co-change window, no labeler
    /// (falls back to the longest-common-prefix heuristic), and no cache dir.
    /// `seed` defaults to 0 — callers MUST override it from `BuildOptions.seed`
    /// (P0.1) for cross-run determinism.
    fn default() -> Self {
        Self {
            seed: 0,
            k_candidates: vec![8, 16, 32, 64],
            min_cluster_weight: 0.05,
            lof_k: 20,
            walks_per_node: 10,
            walk_length: 40,
            node2vec_p: 1.0,
            node2vec_q: 1.0,
            cochange_window: Duration::from_secs(90 * 24 * 60 * 60),
            labeler: None,
            cache_dir: None,
        }
    }
}

/// Cluster labeler port (P1.2's small LLM, or a test stub). The
/// `Debug + Send + Sync` supertrait bounds let `BuildVisionOptions` derive
/// `Debug` and stay shareable across the `spawn_blocking` boundary, so the
/// `Send + Sync` markers no longer need restating at each `dyn` use site.
pub trait LabelGenerator: Debug + Send + Sync {
    /// Produce a 2–5 word concept name for the given member snippets.
    ///
    /// # Errors
    /// Returns [`AnalyzeError::Labeler`] if the underlying model call fails
    /// (network, rate-limit, or empty/garbage output). Implementations should
    /// preserve the source error via `#[source]`.
    fn label_cluster(&self, member_snippets: &[String]) -> Result<String, AnalyzeError>;
}

/// Errors surfaced by the analyze/vision pipeline.
///
/// Variants distinguish the failure modes a caller may branch on: bad input
/// (skip/abort), numeric failure (retry with different k / fall back to
/// spectral), VCS I/O (degrade co-change to empty), and labeler failure
/// (degrade to the heuristic label). `anyhow` is confined to the binaries.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AnalyzeError {
    /// Invalid or insufficient input (empty embedding set, dimension mismatch,
    /// fewer items than the smallest requested k).
    #[error("invalid analyze input: {0}")]
    InvalidInput(String),

    /// Numeric failure during clustering / scoring: rank-deficient covariance,
    /// GMM non-convergence, non-finite likelihood, or a guarded divide-by-zero
    /// in softmax / Mahalanobis / PMI normalization.
    #[error("numeric failure during {stage}: {detail}")]
    Numeric { stage: &'static str, detail: String },

    /// VCS subprocess / parse failure while building the co-change index.
    #[error("vcs i/o failure building co-change index")]
    Vcs(#[source] std::io::Error),

    /// Underlying storage / snapshot read failure.
    #[error("snapshot read failure")]
    Snapshot(#[source] heed::Error),

    /// Cluster labeler (LLM) failure; the build degrades to heuristic labels.
    #[error("cluster labeler failed")]
    Labeler(#[source] Box<dyn std::error::Error + Send + Sync>),
}
```

```rust
// crates/rmc-graph/src/analyze/features.rs

// d_struct = 1 (in_deg log) + 1 (out_deg log) + 1 (module_depth) + 11 (item_kind one-hot) + 1 (attr_bits) = 15
// d_embed = backend dim (1024 for Qwen3-0.6B)

#[derive(Debug, Clone)]
pub struct FeatureVector {
    pub embedding: Vec<f32>,
    pub structural: StructuralFeatures,
}
impl FeatureVector {
    pub fn dim(&self) -> usize { self.embedding.len() + StructuralFeatures::DIM }
    pub fn flatten(&self, w_embed: f32, w_struct: f32) -> Vec<f32>;
}

#[derive(Debug, Clone, Copy)]
pub struct StructuralFeatures {
    pub in_deg: u32,
    pub out_deg: u32,
    pub module_depth: u32,
    pub kind_onehot: [f32; 11],   // Function, Struct, Enum, Union, Trait, TypeAlias, Const, Static, AssocFunction, AssocType, EnumVariant
                                  // (Method collapsed → AssocFunction, AssocConst → Const)
    pub attr_bits: u32,            // bitset: must_use, non_exhaustive, deprecated, derive_present, repr_present, doc_present, async_present, unsafe_present, inline_present, allow_present, target_feature_present, ...
}
impl StructuralFeatures { pub const DIM: usize = 1 + 1 + 1 + 11 + 1; }

// NOTE: `build_vision` (the public entry point) and `EmbeddingsLookup` live in
// the crate-entry file `analyze.rs`, not here — per file-based-module layout
// the entry surface belongs at the module root, while `features.rs` keeps only
// the feature-building internals (`build_features`, `FeatureVector`,
// `StructuralFeatures`).
```

```rust
// crates/rmc-graph/src/analyze.rs  (entry point — moved out of features.rs)

/// Build the full vision index (features → cluster → outliers → affinity →
/// co-change → labels → cache) for one published snapshot.
///
/// # Errors
/// - [`AnalyzeError::InvalidInput`] if `embeds` is empty, dimensions mismatch,
///   or there are fewer items than the smallest `opts.k_candidates`.
/// - [`AnalyzeError::Numeric`] on rank-deficiency / non-convergence that even
///   the spectral fallback cannot recover.
/// - [`AnalyzeError::Vcs`] / [`AnalyzeError::Snapshot`] / [`AnalyzeError::Labeler`]
///   propagated from co-change, storage reads, and labeling respectively
///   (labeling failures degrade to heuristic labels rather than aborting).
pub fn build_vision(
    snap: &OpenedSnapshot,
    embeds: &EmbeddingsLookup<'_>,
    opts: &BuildVisionOptions,
) -> Result<VisionIndex, AnalyzeError>;

pub struct EmbeddingsLookup<'a> {
    pub by_target: &'a HashMap<NodeId, Vec<f32>>,
    pub embedder_dim: usize,
}
```

## NaN / Inf policy and numeric guards (§7/§9)

Floating-point fields are kept finite by construction; raw library output is
sanitized before it reaches any public type. `f32` is not `Ord`, so every
argmin / sort over float keys uses `f64::total_cmp` (promote `f32` to `f64`
first). Specific rules:

- **`silhouette: f32` / `silhouette_mean`** — a singleton or empty cluster has
  no defined silhouette; coerce any non-finite value (`NaN`/`±Inf`) to `0.0`
  before storing. `Cluster.davies_bouldin_contrib` and `VisionQuality.davies_bouldin`
  coerce `NaN` to `f32::INFINITY` (worst separation), never `NaN`.
- **BIC argmin (`picked_k`)** — drop any `(k, bic)` whose `bic` is non-finite
  from the candidate set; pick `argmin` via `f64::total_cmp`. If *all* candidates
  are non-finite, return `AnalyzeError::Numeric { stage: "bic" , .. }` (callers
  fall back to spectral).
- **Softmax (spectral membership, entropy)** — subtract the row max before
  `exp` to avoid overflow; if the denominator underflows to `0.0`, fall back to
  a uniform distribution over the active clusters (never divide by zero).
- **Mahalanobis** — skip clusters with `< 2·d` members or a rank-deficient /
  zero-determinant covariance (regularize with `+ εI`, `ε = 1e-6`); a guarded
  failure yields no findings for that cluster rather than `NaN`.
- **PMI / co-change normalization** — add-one (Laplace) smoothing guarantees
  non-zero marginals, so `log(p(a,b)/(p(a)·p(b)))` is always finite; clamp the
  result with `is_finite()` and keep positives only.
- **Zero-variance / zero-norm guards** — ℓ2 block normalization checks the norm
  is `> 0` before dividing (zero-norm rows pass through unscaled);
  per-feature zero variances are floored to `ε` before standardization.

## Step-by-step implementation

1. **Item-feature builder.** WHERE: `analyze/features.rs`. Single `RoTxn`. Iterate `nodes_by_id` keeping `NodeKind::Item` with `file.is_some() && span.is_some()` (reuse `enumerate_similarity_seeds` from `query/similarity.rs`). For each, fetch vector from `EmbeddingsLookup.by_target` (skip if absent — callers pre-populate via `embedding_cache::ensure_embeddings_for`). Structural: `in_deg = usages_by_target.prefix(nid).count()`, `out_deg = usages_by_consumer_function.prefix(nid).count()` (fall back to `usages_by_consumer.prefix(parent_module).filter(consumer_function)` for non-fn items); `module_depth` walks `parent_id` chain; `kind_onehot` from `Node.item_kind` (collapse Method→AssocFunction, AssocConst→Const); `attr_bits` from `snap.item_attributes(nid)` matching known markers. `log1p` in/out degrees to compress tail. VERIFY: `feature_vector_shape`.

2. **Joined-substrate matrix.** WHERE: `cluster.rs::assemble_matrix`. `ndarray::Array2<f32>` shape `n × (d_embed + d_struct)`. ℓ2-normalize each block; multiply by per-block weights (1.0 embedding, 0.5 structural). Keep `row_to_nid: Vec<NodeId>`. VERIFY: `assemble_matrix_row_count`.

3. **GMM clustering with BIC sweep.** Use `linfa_clustering::GaussianMixtureModel::params(k).with_rng(rng).max_n_iterations(200).tolerance(1e-3).init_method(GmmInitMethod::KMeansPlusPlus)`. RNG = `rand_chacha::ChaCha8Rng::seed_from_u64(opts.seed.wrapping_add(k as u64))` per-k (step 12). For each `k ∈ opts.k_candidates`: fit; compute log-likelihood + BIC = `−2·ll + k·log(n)·d_params` (diagonal covariance variant if d > 256). Drop any `(k, bic)` with non-finite `bic`; `picked_k = argmin BIC` via `f64::total_cmp` (all non-finite → `AnalyzeError::Numeric { stage: "bic", .. }`, caller falls back to spectral). Re-fit; soft membership from `predict_proba`; apply `min_cluster_weight` floor. VERIFY: `cluster_count_under_bic`.

4. **Spectral fallback.** WHEN GMM rejected (picked_k at max-k with monotonic BIC, suggests under-fitting) OR rank-deficient. Build undirected `petgraph::Graph<NodeId, f32>` from call/usage adjacency. Symmetric normalized Laplacian as dense `nalgebra::DMatrix<f64>` (workspaces top at ~5k items; dense fine). `L.symmetric_eigen()` → bottom-k eigenvectors → `Y ∈ R^{n×k}` → seeded `linfa_clustering::KMeans` → map back via `softmax(−|y − μ|² / τ)`.

5. **Default = GMM** (direct soft membership for Issue #9 zoom-through). Spectral wired as `BuildVisionOptions::clustering = ClusteringKind::Spectral`.

6. **Outliers.** LOF via `linfa_anomaly::LocalOutlierFactor::params(opts.lof_k).fit(&matrix)?.predict_score(&matrix)`; above 95th percentile → `LocalLOF`. Per-cluster Mahalanobis: clusters with ≥ `2·d` members; compute sample mean + covariance via `nalgebra`; for points with `H(p) > log(picked_k)·0.8` (high entropy), Mahalanobis to dominant centroid; above 97.5th percentile → `Mahalanobis`. Items absent from `assignment` after floor → `UnclusteredLone`. VERIFY: `outlier_finds_planted`.

7. **AffinityIndex.** node2vec-ish PMI: build `petgraph::DiGraph<NodeId, f32>` from `usages_by_consumer_function` + `bindings_by_target`. For each Item, `walks_per_node` walks of length `walk_length` with biased step (prob ∝ `1/p` revisit, `1/q` farther). RNG = `ChaCha8Rng::seed_from_u64(stable_node_seed(opts.seed, nid))` for per-node determinism (the `stable_node_seed` helper is defined in step 12 — it is NOT `DefaultHasher`/`ahash`). Accumulate co-occurrence in window 5 along each walk. After all walks: `PMI(a,b) = log(p(a,b) / (p(a) · p(b)))` with add-one smoothing (finite by construction; clamp `is_finite()`). Keep positive only; canonicalize keys. VERIFY: `affinity_directional_invariant`.

8. **CoChangeIndex.** Detect VCS: `.jj` present → use jj; else `.git` → git; else empty.
   - jj: `Command::new("jj").args(["log", "-r", &format!("ancestors(@) & description(glob:'*') & after({})", since_iso8601), "-T", r#"separate(" ",commit_id,"\n",files.map(|f| f.path()))"#, "--no-graph"]).output()`.
   - git fallback: `git log --since="<duration>" --name-only --pretty=format:"COMMIT %H"`.
   Parse into `Vec<HashSet<String>>`. File → NodeId map from `nodes_by_id` where `Node.file == path`. Co-change between files becomes cross-product of items in each commit weighted by `1/(|set_a|·|set_b|)`. `log(p(a∧b) / (p(a)·p(b)))` with add-one smoothing. Sparse canonical-key. WHERE: `tokio::task::spawn_blocking`. VERIFY: `cochange_from_synthetic_history`.

9. **LLM cluster labels.** Top-K (default 5) members per cluster by membership. Look up source slice via P1.2's `embedding_cache::prepare_embeddings_for` recipe (file, span, trim, ~400 chars cap). If `opts.labeler.is_some()`: prompt `"Name the concept these {N} Rust items share in 2 to 5 words. Output only the name.\n\n{snippets}"`; a labeler error (`AnalyzeError::Labeler`) is logged and the cluster degrades to the fallback rather than aborting the whole build. Sanitize then wrap into `ClusterLabel` (trim, collapse whitespace, cap to 5 words). Else fallback: longest common qualified-name prefix beyond crate root → modal item kind label. VERIFY: `labels_are_short`.

10. **`VisionIndex` assembly + cache.** Resolve the `CommitHash` via `jj log -r @ -T 'commit_id'` or `git rev-parse HEAD`; `CommitHash::is_empty()` disables cache. Key = `format!("{}_{}", graph_id.as_str(), head_commit_hash.as_str())`. Read: `cache_dir.join(format!("{key}.json"))` — early return if exists. Write: `serde_json::to_writer_pretty` to `tempfile::NamedTempFile::persist`. **Flat JSON over LMDB** because cache rows are large, per-episode, and ride D1's working-dir convention.

11. **`MapPane::Cluster` integration.**
    ```rust
    impl VisionIndex {
        pub fn clusters_at_zoom(&self, scale: f32) -> Vec<ClusterId>;  // scale ∈ [0,1]
        pub fn clusters_for_node(&self, node: NodeId) -> Vec<(ClusterId, f32)>;
    }
    ```

12. **Determinism plumbing.** Every RNG derives from `opts.seed` (from `BuildOptions.seed`, P0.1) through a **stable, version-independent** scheme — `DefaultHasher` and `ahash` are explicitly forbidden because their output is not stable across runs, std versions, or builds, which would break `seeded_clustering_stable`. Concretely:
    - **Top-level RNG:** `ChaCha8Rng::seed_from_u64(opts.seed)`.
    - **Per-k derivation:** `ChaCha8Rng::seed_from_u64(opts.seed.wrapping_add(k as u64))` — a fixed, reproducible offset chain (the `wrapping_add(k)` arithmetic is itself stable; no hashing involved).
    - **Per-node walk seed:** a single fixed helper, the *only* place `(seed, nid)` is folded into a `u64`, using SipHash-1-3 with a **hard-coded constant key** (not the randomized `RandomState`):
      ```rust
      use siphasher::sip::SipHasher13;   // siphasher = "1", deterministic, no RNG key
      use std::hash::Hasher;

      /// Stable per-node seed. Fixed key ⇒ identical across runs, machines,
      /// and toolchain versions, unlike `DefaultHasher`/`ahash`.
      fn stable_node_seed(seed: u64, nid: NodeId) -> u64 {
          let mut h = SipHasher13::new_with_keys(0x5111_1ed5_eed0_0001, seed);
          h.write(nid.as_bytes());        // NodeId's stable 16-byte content id
          h.finish()
      }
      ```
    - **Linfa:** pass the seeded `ChaCha8Rng` via `.with_rng(rng)`.

    This makes the seed→RNG mapping a pure function of `(opts.seed, nid)`, so two builds with the same seed produce byte-identical assignments and outliers. VERIFY: `seeded_clustering_stable`.

13. **Incremental update on P0.2 affected set.** Do NOT recluster. Recompute features for affected items + new items. Assign to nearest existing cluster by evaluating each cluster's stored Gaussian (centroid persisted; cov recomputed from members on demand). Apply `min_cluster_weight` floor. Update `assignment` in place; write new cache file. Track drift: recompute `silhouette_mean` from affected rows; if drops > 0.1, `tracing::warn!` "cluster quality drift". Full recluster at episode end (build_vision entry), not incremental. VERIFY: `incremental_update_stable_for_unaffected`.

## Tests

(`crates/rmc-graph/src/analyze/tests.rs`, gated on `analyze` feature)

- **`feature_vector_shape`** — `dim() == 1024 + 15`; `flatten(1.0, 0.5).len() == 1039`; zero-norm guarded.
- **`structural_features_pulls_correct_degrees`** — 3-node synthetic graph; in_deg.log1p() ≈ ln(3).
- **`cluster_count_under_bic`** — plant 3 Gaussians 5σ apart at d=8; assert `picked_k == 3`.
- **`seeded_clustering_stable`** — two builds same seed → identical assignment + outliers.
- **`outlier_finds_planted`** — plant `embedding = vec![20.0; d]`; flagged with `LocalLOF` and max score.
- **`affinity_directional_invariant`** — 4-cycle; `score(a,b) == score(b,a)` for all pairs.
- **`cochange_from_synthetic_history`** — synthetic git repo: c1={A}, c2={A,B}, c3={B,C}; `score(A,B) > 0`, `score(A,C) ≈ 0`.
- **`cochange_handles_mega_commit`** — 200-file commit yields smaller per-pair than 2-file commit.
- **`labels_are_short`** — fallback + stub LabelGenerator both produce ≤ 5 words.
- **`incremental_update_stable_for_unaffected`** — original NodeIds' assignment byte-identical after adding 1 new item.
- **`vision_cache_round_trip`** — write, drop, re-read; serde_json round-trip equality.
- **`vision_cache_key_changes_with_commit_hash`**.
- **`zoom_through_returns_raw_nodes`** — 3 clusters of 10; `clusters_at_zoom(0.0).len() <= 3`; `clusters_for_node(nid)` returns top cluster with weight > 0.5.
- **`end_to_end_on_shared_snapshot`** — `test_support::shared_snapshot()`, `k_candidates: vec![4]`, `walks_per_node: 2`; build < 30s, clusters non-empty, silhouette > 0.

## Open decisions / risks

- **Feature engineering** — commits to: ℓ2-normalize embedding+structural separately; concat with 1.0/0.5; d_struct = 15; collapse Method→AssocFunction + AssocConst→Const. Open: add `signature.params.len()` + `signature.generics.len()` after BIC curves stabilize.
- **Clustering library** — `linfa-clustering` + `linfa-anomaly` (pure Rust, no Python). HDBSCAN drops soft membership → kills Issue #9 mitigation. `nalgebra` dense for spectral fallback (5k Items → 200MB Laplacian fits).
- **Co-change window** — 90 days default; `BuildVisionOptions::cochange_window`. < 30 days history → `tracing::warn!`; treat scores as low-confidence.
- **VCS detection precedence** — jj first (rmc uses jj per AGENTS.md), git second. CI must exercise jj branch.
- **Cluster quality monitoring** — silhouette + Davies-Bouldin per build in `VisionQuality`. Warn on > 0.1 drop between episodes. Future P1.3.x feeds metric stream into P1.7 reward.
- **Reclustering policy** — incremental assign mid-episode; full refit at episode end. Bounds per-step cost at O(k·d·|affected|). Risk: long episodes drift; mitigation via silhouette drift warning + forced full refit.
- **Where labels live** — `cluster_labels` JSON sidecar in cache file. NOT mixed with P1.2 descriptions (different ID space, ephemeral per-episode). Promote to LMDB sub-DB if survive episodes is needed.
- **LLM label cost** — 32 clusters × ~500ms ≈ 16s added per build. Mitigation: `futures::future::join_all` inside `LabelGenerator` adapter.
- **Determinism of jj/git output** — sort parsed commits by `(commit_id, files)` before consuming.
- **`min_cluster_weight`** — 0.05 default; truncates 99% of trailing memberships. Calibrate after M1 dogfooding.
- **Cache invalidation on embedder change** — add `backend_identity: Option<String>` to `BuildVisionOptions`; include in cache filename. Avoids coupling `analyze` to `rmc-engine`.


---

