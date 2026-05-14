# 09 — Embedding placement

## Decision: Option C — extract a narrow `embedding-runtime` crate

`embedding-runtime` owns the ONNX/ort/CUDA model lifecycle and exposes a small text-in / vectors-out surface. Both `code-search` and `graph` depend on it. No trait injection theater, no duplication, no DAG-violating back-edge from `graph` to `code-search`.

## Why C beats the others

- **A (graph -> code-search)** inverts the layering. `code-search` is a query/orchestration crate that itself depends on indexing, vector_store, chunker, parser. Letting `graph` reach into it pulls Tantivy + LanceDB transitively into the audit/snapshot crate. The "graph !-> code-search" test is enforcing a real invariant; do not break it.
- **B (trait injection)** sounds clean but is worse than C in practice. `EmbedTexts` would have to be defined *somewhere* both crates depend on, which is exactly the narrow crate C proposes — except B then forces the runtime to live inside `code-search`, so wiring `graph` requires the binary crate to construct an adapter and thread it through every snapshot-opening path. You pay the abstraction tax (dyn dispatch, lifetimes on the trait, async-trait gymnastics for `embed_async`) and still need the runtime crate for the trait alone.
- **D (duplicate, two ONNX sessions)** doubles cold-load cost (~250-400 ms each), doubles CUDA VRAM residency (5.5 GB cap is configured per session), and lets the two sessions drift on model version, tokenizer, normalization, and provider order. Non-starter for a 384-dim shared embedding contract.
- **E** — none of the obvious alternatives (out-of-process embedder, HTTP sidecar, lazy-init via `OnceCell` in a leaf crate) beat C without adding IPC or hidden global state.

C is the minimum thing that lets one ONNX session be shared by both consumers under the existing dependency rules.

## API shape

Crate `embedding-runtime`. No `pub use` of `fastembed`, `ort`, `ndarray`, or any CUDA type.

```rust
pub struct Embedder { /* Arc<Mutex<TextEmbedding>>, dimensions: usize */ }

#[derive(Debug, Clone)]
pub struct EmbedderConfig {
    pub model: ModelKind,           // enum { AllMiniLmL6V2 } — extensible, opaque
    pub cuda: CudaPolicy,           // enum { Auto, Force, Disabled }
    pub cuda_mem_limit_bytes: u64,  // default 5.5 GiB
}

#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("model init failed: {0}")] ModelInit(String),
    #[error("embed failed: {0}")]      EmbedFailed(String),
    #[error("no embedding produced")]  Empty,
    #[error("blocking task join failed: {0}")] Join(String),
}

impl Embedder {
    pub fn new(cfg: EmbedderConfig) -> Result<Self, EmbedError>;
    pub fn dimensions(&self) -> usize;

    pub fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedError>;
    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError>;

    pub async fn embed_async(self: &Arc<Self>, text: String)
        -> Result<Vec<f32>, EmbedError>;
    pub async fn embed_batch_async(self: &Arc<Self>, texts: Vec<String>)
        -> Result<Vec<Vec<f32>>, EmbedError>;
}
```

No lifetimes leak: inputs are `&str` / `&[&str]` / owned `String` for async; outputs are owned `Vec<f32>`. `Arc<Embedder>` is the sharing primitive — same instance handed to `code-search::SearchService` and to `graph::semantic_overlaps`.

## Resource lifecycle

- **One ONNX session, process-wide.** Constructed once in `main` (or via a `OnceLock<Arc<Embedder>>` in the binary crate), passed by `Arc` into `SearchService::new` and into the graph query entry points that need it.
- **Cold load.** Single `TextEmbedding::try_new` (~250-400 ms, ~120 MB resident, ~5.5 GB CUDA reservation). Re-loading a second session for graph would double both. C eliminates that.
- **Serialization.** Internal `Mutex<TextEmbedding>` keeps inference single-threaded (matches today's `EmbeddingGenerator`). Async paths use `spawn_blocking`. Mutex poisoning is fatal.
- **CUDA residency.** One execution-provider chain (CUDA -> CPU). VRAM cap configured at construction; no runtime fallback once chosen.

## Hiding ONNX/ort/CUDA

- `embedding-runtime` does not re-export anything from `fastembed` or `ort`. `Cargo.toml` keeps both as private deps.
- Public surface is `Vec<f32>` + a small enum + a typed error. Provider configuration is expressed via `CudaPolicy`, never an `ort` type.
- Downstream crates (`code-search`, `graph`) compile without `fastembed` / `ort` in their dependency closure's *direct* graph; cargo deduplicates the transitive ONNX session crate.

## Measurement gate (must pass before adoption)

Bench harness (one criterion run, gated in CI as a non-flaky smoke):
1. Cold start: `Embedder::new` p50 <= 600 ms on the dev box.
2. Single-session resident memory after `embed_batch` of 256 chunks: < 200 MB host + <= configured CUDA cap.
3. Across 1k sequential `embed_async` calls from `code-search` and 1k from `graph::semantic_overlaps` on the same `Arc<Embedder>`: zero second model load events (assert via tracing span counter), p95 latency within 5% of today's `EmbeddingGenerator` p95.
4. `cargo tree -p graph` shows no `fastembed` / `ort` / `lancedb` / `tantivy` direct edges; only `embedding-runtime`.

If (1)-(4) pass, adopt. If any fail, reassess B with async-trait erasure.

## Top 3 risks

1. **Crate proliferation.** One more workspace member to version, document, and test. Mitigation: keep it small (<500 LoC), no public re-exports, single error type.
2. **Model-config drift between callers.** `code-search` and `graph` could disagree on `EmbedderConfig`. Mitigation: construct the `Arc<Embedder>` in `main` only; downstream APIs accept `Arc<Embedder>`, never an `EmbedderConfig`.
3. **Hidden coupling via vector dimensionality.** Both consumers assume 384-dim. Changing the model breaks LanceDB schema and `semantic_overlaps`. Mitigation: expose `Embedder::dimensions()` and make consumers assert at startup; bump LanceDB schema version on change.
