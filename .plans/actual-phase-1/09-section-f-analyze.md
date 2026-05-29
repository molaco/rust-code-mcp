# Section F — P1.3 Analyze / Vision Layer

## Overview

This slice introduces an `analyze` subtree inside `rmc-graph` that turns the per-Item embedding cache (`embeddings_by_target`) and the call/usage substrates into the mesoscale "city map" the navigator (P1.1) renders at the cluster zoom level. It computes per-Item joined feature vectors (Qwen3 embedding ⊕ structural features), clusters them with GMM over the joined substrate (with a spectral fallback over the Laplacian of the call graph), labels each cluster with a 2–5 word concept name produced by the same small LLM that P1.2 uses, scores per-Item outliers (LOF + per-cluster Mahalanobis), and emits two queryable affinity scores (`affinity`: random-walk PMI on the call/usage graph; `co_change`: file-co-occurrence lift from `jj log` / `git log --name-only`). All outputs land in a single `VisionIndex` value cached per `(graph_id, head_commit_hash)`.

M1 work, runs against the slow published snapshot, no P0.2 dependency. Load-bearing for perception (Issue #9): (a) soft membership, not hard assignments; (b) zoom-through to raw nodes via `assignment: HashMap<NodeId, Vec<(ClusterId, f32)>>`; (c) silhouette + Davies–Bouldin per build for quality monitoring; (d) deterministic `seed` from `BuildOptions` threaded through every random source. Fills P1.1's "cluster scale stub" via `clusters_at_zoom(scale)` that the navigator's `MapPane::Cluster` layer reads.

## New modules / files

- `crates/rmc-graph/src/analyze/mod.rs` — public surface + `build_vision()` entry: `ClusterId`, `Cluster`, `OutlierFinding`, `OutlierKind`, `AffinityIndex`, `CoChangeIndex`, `VisionIndex`, `BuildVisionOptions`.
- `crates/rmc-graph/src/analyze/features.rs` — `FeatureVector`, `StructuralFeatures`, `build_features()`.
- `crates/rmc-graph/src/analyze/cluster.rs` — GMM via `linfa-clustering::GaussianMixtureModel` + spectral fallback (`petgraph` Laplacian + `nalgebra` symmetric eigen + k-means).
- `crates/rmc-graph/src/analyze/outliers.rs` — LOF via `linfa-anomaly::LocalOutlierFactor` + per-cluster Mahalanobis using `nalgebra`.
- `crates/rmc-graph/src/analyze/affinity.rs` — biased random walks on merged `petgraph` graph; pair-count → PMI.
- `crates/rmc-graph/src/analyze/cochange.rs` — async wrapper shelling out to `jj log` (preferred) or `git log --name-only` (fallback).
- `crates/rmc-graph/src/analyze/labels.rs` — LLM-based cluster labeling via P1.2's model handle.
- `crates/rmc-graph/src/analyze/cache.rs` — per-episode cache keyed `(graph_id, head_commit_hash)`. JSON files under `working/<session_id>/vision/<key>.json`.
- `crates/rmc-graph/src/analyze/zoom.rs` — `clusters_at_zoom(scale)` for `MapPane::Cluster`.
- `crates/rmc-graph/src/lib.rs` — `pub mod analyze;`.
- `crates/rmc-graph/Cargo.toml` — new `analyze` feature pulling `petgraph = "0.6"`, `linfa = "0.7"`, `linfa-clustering = "0.7"`, `linfa-anomaly = "0.7"`, `nalgebra = "0.33"`, `ndarray = "0.16"`, `rand = "0.8"`, `rand_chacha = "0.8"`.

## Type definitions

```rust
// crates/rmc-graph/src/analyze/mod.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClusterId(pub u32);
// Derived by hashing (graph_id, head_commit_hash, "cluster", local_idx)

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cluster {
    pub id: ClusterId,
    pub members: Vec<(NodeId, f32)>,    // (id, soft_membership), sorted desc
    pub centroid: Vec<f32>,
    pub silhouette: f32,
    pub davies_bouldin_contrib: f32,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutlierKind { LocalLOF, Mahalanobis, UnclusteredLone }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutlierFinding { pub item: NodeId, pub cluster: ClusterId, pub score: f32, pub kind: OutlierKind }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffinityIndex { pairs: HashMap<(NodeId, NodeId), f32> }
impl AffinityIndex { pub fn score(&self, a: NodeId, b: NodeId) -> f32; }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoChangeIndex { pairs: HashMap<(NodeId, NodeId), f32>, pub window: Duration }
impl CoChangeIndex {
    pub fn score(&self, a: NodeId, b: NodeId) -> f32;
    pub async fn build_from_vcs(snap: &OpenedSnapshot, workspace_root: &Path, window: Duration) -> Result<Self>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionIndex {
    pub graph_id: String,
    pub head_commit_hash: String,
    pub clusters: Vec<Cluster>,
    pub assignment: HashMap<NodeId, Vec<(ClusterId, f32)>>,
    pub outliers: Vec<OutlierFinding>,
    pub affinity: AffinityIndex,
    pub co_change: CoChangeIndex,
    pub quality: VisionQuality,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionQuality {
    pub silhouette_mean: f32,
    pub davies_bouldin: f32,
    pub picked_k: usize,
    pub bic_curve: Vec<(usize, f64)>,
}

#[derive(Debug, Clone)]
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
    pub labeler: Option<Arc<dyn LabelGenerator + Send + Sync>>,
    pub cache_dir: Option<PathBuf>,
}

pub trait LabelGenerator {
    fn label_cluster(&self, member_snippets: &[String]) -> Result<String>;
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

pub fn build_vision(snap: &OpenedSnapshot, embeds: &EmbeddingsLookup, opts: &BuildVisionOptions) -> Result<VisionIndex>;

pub struct EmbeddingsLookup<'a> {
    pub by_target: &'a HashMap<NodeId, Vec<f32>>,
    pub embedder_dim: usize,
}
```

## Step-by-step implementation

1. **Item-feature builder.** WHERE: `analyze/features.rs`. Single `RoTxn`. Iterate `nodes_by_id` keeping `NodeKind::Item` with `file.is_some() && span.is_some()` (reuse `enumerate_similarity_seeds` from `query/similarity.rs`). For each, fetch vector from `EmbeddingsLookup.by_target` (skip if absent — callers pre-populate via `embedding_cache::ensure_embeddings_for`). Structural: `in_deg = usages_by_target.prefix(nid).count()`, `out_deg = usages_by_consumer_function.prefix(nid).count()` (fall back to `usages_by_consumer.prefix(parent_module).filter(consumer_function)` for non-fn items); `module_depth` walks `parent_id` chain; `kind_onehot` from `Node.item_kind` (collapse Method→AssocFunction, AssocConst→Const); `attr_bits` from `snap.item_attributes(nid)` matching known markers. `log1p` in/out degrees to compress tail. VERIFY: `feature_vector_shape`.

2. **Joined-substrate matrix.** WHERE: `cluster.rs::assemble_matrix`. `ndarray::Array2<f32>` shape `n × (d_embed + d_struct)`. ℓ2-normalize each block; multiply by per-block weights (1.0 embedding, 0.5 structural). Keep `row_to_nid: Vec<NodeId>`. VERIFY: `assemble_matrix_row_count`.

3. **GMM clustering with BIC sweep.** Use `linfa_clustering::GaussianMixtureModel::params(k).with_rng(rng).max_n_iterations(200).tolerance(1e-3).init_method(GmmInitMethod::KMeansPlusPlus)`. RNG = `rand_chacha::ChaCha8Rng::seed_from_u64(opts.seed)`. For each `k ∈ opts.k_candidates`: fit; compute log-likelihood + BIC = `−2·ll + k·log(n)·d_params` (diagonal covariance variant if d > 256). `picked_k = argmin BIC`. Re-fit; soft membership from `predict_proba`; apply `min_cluster_weight` floor. VERIFY: `cluster_count_under_bic`.

4. **Spectral fallback.** WHEN GMM rejected (picked_k at max-k with monotonic BIC, suggests under-fitting) OR rank-deficient. Build undirected `petgraph::Graph<NodeId, f32>` from call/usage adjacency. Symmetric normalized Laplacian as dense `nalgebra::DMatrix<f64>` (workspaces top at ~5k items; dense fine). `L.symmetric_eigen()` → bottom-k eigenvectors → `Y ∈ R^{n×k}` → seeded `linfa_clustering::KMeans` → map back via `softmax(−|y − μ|² / τ)`.

5. **Default = GMM** (direct soft membership for Issue #9 zoom-through). Spectral wired as `BuildVisionOptions::clustering = ClusteringKind::Spectral`.

6. **Outliers.** LOF via `linfa_anomaly::LocalOutlierFactor::params(opts.lof_k).fit(&matrix)?.predict_score(&matrix)`; above 95th percentile → `LocalLOF`. Per-cluster Mahalanobis: clusters with ≥ `2·d` members; compute sample mean + covariance via `nalgebra`; for points with `H(p) > log(picked_k)·0.8` (high entropy), Mahalanobis to dominant centroid; above 97.5th percentile → `Mahalanobis`. Items absent from `assignment` after floor → `UnclusteredLone`. VERIFY: `outlier_finds_planted`.

7. **AffinityIndex.** node2vec-ish PMI: build `petgraph::DiGraph<NodeId, f32>` from `usages_by_consumer_function` + `bindings_by_target`. For each Item, `walks_per_node` walks of length `walk_length` with biased step (prob ∝ `1/p` revisit, `1/q` farther). RNG = `ChaCha8Rng` seeded with `(opts.seed, nid)` for per-node determinism. Accumulate co-occurrence in window 5 along each walk. After all walks: `PMI(a,b) = log(p(a,b) / (p(a) · p(b)))` with add-one smoothing. Keep positive only; canonicalize keys. VERIFY: `affinity_directional_invariant`.

8. **CoChangeIndex.** Detect VCS: `.jj` present → use jj; else `.git` → git; else empty.
   - jj: `Command::new("jj").args(["log", "-r", &format!("ancestors(@) & description(glob:'*') & after({})", since_iso8601), "-T", r#"separate(" ",commit_id,"\n",files.map(|f| f.path()))"#, "--no-graph"]).output()`.
   - git fallback: `git log --since="<duration>" --name-only --pretty=format:"COMMIT %H"`.
   Parse into `Vec<HashSet<String>>`. File → NodeId map from `nodes_by_id` where `Node.file == path`. Co-change between files becomes cross-product of items in each commit weighted by `1/(|set_a|·|set_b|)`. `log(p(a∧b) / (p(a)·p(b)))` with add-one smoothing. Sparse canonical-key. WHERE: `tokio::task::spawn_blocking`. VERIFY: `cochange_from_synthetic_history`.

9. **LLM cluster labels.** Top-K (default 5) members per cluster by membership. Look up source slice via P1.2's `embedding_cache::prepare_embeddings_for` recipe (file, span, trim, ~400 chars cap). If `opts.labeler.is_some()`: prompt `"Name the concept these {N} Rust items share in 2 to 5 words. Output only the name.\n\n{snippets}"`. Sanitize: trim, collapse whitespace, cap to 5 words. Else fallback: longest common qualified-name prefix beyond crate root → modal item kind label. VERIFY: `labels_are_short`.

10. **`VisionIndex` assembly + cache.** Resolve `head_commit_hash` via `jj log -r @ -T 'commit_id'` or `git rev-parse HEAD`; empty disables cache. Key = `format!("{}_{}", graph_id, head_commit_hash)`. Read: `cache_dir.join(format!("{key}.json"))` — early return if exists. Write: `serde_json::to_writer_pretty` to `tempfile::NamedTempFile::persist`. **Flat JSON over LMDB** because cache rows are large, per-episode, and ride D1's working-dir convention.

11. **`MapPane::Cluster` integration.**
    ```rust
    impl VisionIndex {
        pub fn clusters_at_zoom(&self, scale: f32) -> Vec<ClusterId>;  // scale ∈ [0,1]
        pub fn clusters_for_node(&self, node: NodeId) -> Vec<(ClusterId, f32)>;
    }
    ```

12. **Determinism plumbing.** Every RNG: `ChaCha8Rng::seed_from_u64(opts.seed)` for top-level; `seed.wrapping_add(k)` per-k; `hash64((seed, nid))` per-node walks. Linfa: `.with_rng(rng)`. `opts.seed` from `BuildOptions.seed` (P0.1). VERIFY: `seeded_clustering_stable`.

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

