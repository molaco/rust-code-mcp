//! Embedding generation using fastembed's Candle backend (Qwen3).
//!
//! `EmbeddingGenerator` wraps a `Qwen3Embedder`. The synchronous
//! ONNX path is gone: every public method is `async` and runs the
//! underlying blocking Candle call on the tokio blocking pool.
//!
//! The public surface splits document- and query-side embedding so
//! Qwen3's instruction tuning is applied correctly:
//! - `embed_documents` — raw text, no instruction prefix. Used by the
//!   indexer / cache / batcher.
//! - `embed_queries` — instruction prefix applied. Used by search.

mod error;
pub use error::EmbeddingError;

mod backend;
pub use backend::{EmbeddingBackend, EmbeddingRuntime};

mod profile;
pub use profile::{EmbeddingProfile, Qwen3Variant};
pub use profile::{FastembedOnnxModel, LocalLoaderSpec, QueryPolicy};

mod identity;

pub mod batching;
mod util;

mod profile_registry;
pub use profile_registry::resolve_profile;

mod ep_census;
pub use ep_census::{ProviderCensus, CPU_EP, DIRECTML_EP, MIGRAPHX_EP};

mod kernel_cache;

mod gpu_lock;

mod vram_ledger;
pub use vram_ledger::{
    gpu_ledger_report, read_vram, EmbeddingOwner, GpuLedgerReport, LiveSessionReport,
    OwnerReport, VramDelta, VramReading, VRAM_LOG_TARGET,
};

mod gpu_broker;
pub use gpu_broker::{
    release_idle_gpu_sessions, BrokerReport, BrokerSlotReport, VramBudget, VRAM_BUDGET_ENV,
};

mod fastembed_onnx;

/// Census of graph nodes per execution provider, taken from one profiled
/// run of the selected profile.
///
/// Answers the question `error_on_failure()` does not answer: not
/// whether the EP came up, but whether it got any nodes. Expensive (a separate session;
/// on a cold kernel cache, a MIGraphX compile), so it is called only through an explicit
/// switch, not on every startup.
///
/// Non-fastembed-ONNX profiles (Qwen3/OpenRouter) refuse: they have no
/// ORT session, and therefore no profile to compute the census from.
pub fn probe_provider_census(
    backend: &EmbeddingBackend,
) -> Result<ProviderCensus, EmbeddingError> {
    fastembed_onnx::probe_provider_census(backend)
}
mod openrouter;
pub use openrouter::{
    openrouter_runtime_config, OpenRouterEncodingFormat, OpenRouterProviderPreferences,
    OpenRouterProviderSort, OpenRouterRuntimeConfig,
};
#[cfg(feature = "embeddings-cuda")]
mod qwen3;

mod token_lengths;
pub use token_lengths::{EmbeddingTextLen, EmbeddingTokenCounter};

pub const CUDA_CAPABLE_FEATURES_COMPILED: bool = cfg!(feature = "embeddings-cuda");

/// Every GPU backend compiled into this binary, by name.
///
/// [`CUDA_CAPABLE_FEATURES_COMPILED`] answers only "is NVIDIA/candle in?",
/// and reporting *that* as the binary's GPU capability is actively
/// misleading on any other vendor: an AMD build reads as `false` while it
/// happily runs the graph on MIGraphX. Diagnosing a slow index then starts
/// from "this build has no GPU", which is wrong, and the real cause — a
/// session that never set the profile, so it fell to the CPU default — goes
/// unexamined. Cost the opening of this session's investigation.
pub const GPU_BACKENDS_COMPILED: &[&str] = &[
    #[cfg(feature = "embeddings-cuda")]
    "cuda",
    #[cfg(feature = "embeddings-migraphx")]
    "migraphx",
    #[cfg(feature = "embeddings-directml")]
    "directml",
];

use crate::chunker::{ChunkId, CodeChunk};
use std::sync::Arc;

/// An embedding vector. Dimension depends on the active backend
/// (1024 for Qwen3-0.6B by default).
pub type Embedding = Vec<f32>;

/// A chunk paired with its generated embedding.
#[derive(Debug, Clone)]
pub struct ChunkWithEmbedding {
    pub chunk_id: ChunkId,
    pub embedding: Embedding,
}

/// Which side of the model a call is on. The two sides differ only in the
/// query prefix the backend applies, but the non-finite gate has to be able
/// to repeat a call, and repeating it on the wrong side would silently embed
/// documents as queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmbedSide {
    Documents,
    Queries,
}

/// The first non-finite component found in a batch: which vector, which
/// component, and what the model actually returned.
#[derive(Debug, Clone, Copy, PartialEq)]
struct NonFiniteHit {
    index: usize,
    component: usize,
    value: f32,
}

impl NonFiniteHit {
    fn into_error(self, batch_len: usize) -> EmbeddingError {
        EmbeddingError::NonFiniteEmbedding {
            index: self.index,
            batch_len,
            component: self.component,
            value: self.value,
        }
    }
}

/// Locate the first non-finite component in a batch of embeddings.
///
/// Both NaN and ±infinity count: neither survives a cosine comparison in a
/// useful way, and both mean the same thing here — the model returned
/// something that must not reach the index.
fn first_non_finite(embeddings: &[Embedding]) -> Option<NonFiniteHit> {
    embeddings.iter().enumerate().find_map(|(index, embedding)| {
        embedding
            .iter()
            .enumerate()
            .find(|(_, value)| !value.is_finite())
            .map(|(component, value)| NonFiniteHit {
                index,
                component,
                value: *value,
            })
    })
}

/// Whether a non-finite vector from this runtime is worth recomputing
/// one-by-one before giving up.
///
/// True only for the local ONNX GPU runtimes, where the fault is a measured
/// batching artifact of the execution provider (see
/// [`EmbeddingGenerator::guard_finite`]). Qwen3-on-CUDA is deliberately left
/// out: no such measurement exists for it, and a retry policy invented for a
/// fault nobody has observed would turn a hard failure into a quiet one.
fn retries_non_finite(runtime: EmbeddingRuntime) -> bool {
    matches!(
        runtime,
        EmbeddingRuntime::LocalFastembedOnnxMigraphx | EmbeddingRuntime::LocalFastembedOnnxDirectml
    )
}

/// Embedding generator backed by Qwen3 over fastembed's Candle path.
#[derive(Clone)]
pub struct EmbeddingGenerator {
    inner: EmbeddingGeneratorInner,
    backend: EmbeddingBackend,
}

#[derive(Clone)]
enum EmbeddingGeneratorInner {
    #[cfg(feature = "embeddings-cuda")]
    Qwen3(Arc<qwen3::Qwen3Embedder>),
    /// CPU only: a GPU session is never owned by a generator.
    FastembedOnnx(Arc<fastembed_onnx::FastembedOnnxEmbedder>),
    /// A fastembed GPU runtime: the session lives in `gpu_broker`, one per
    /// backend per process, and each run borrows it as `owner`.
    GpuBrokered { owner: EmbeddingOwner },
    OpenRouter(Arc<openrouter::OpenRouterEmbedder>),
}

impl EmbeddingGenerator {
    /// Construct with the default backend (Qwen3-Embedding-0.6B,
    /// max_len=1024, GPU).
    pub fn new(owner: EmbeddingOwner) -> Result<Self, EmbeddingError> {
        Self::with_backend(EmbeddingBackend::default(), owner)
    }

    /// Construct with an explicit backend configuration.
    ///
    /// `owner` is who the session is for; a GPU session is entered in the
    /// VRAM ledger under it (see `vram_ledger`).
    pub fn with_backend(
        backend: EmbeddingBackend,
        owner: EmbeddingOwner,
    ) -> Result<Self, EmbeddingError> {
        let inner = match backend.runtime {
            EmbeddingRuntime::LocalQwen3CandleCuda => {
                #[cfg(feature = "embeddings-cuda")]
                {
                EmbeddingGeneratorInner::Qwen3(Arc::new(qwen3::Qwen3Embedder::new(&backend)?))
                }
                #[cfg(not(feature = "embeddings-cuda"))]
                {
                    return Err(EmbeddingError::gpu_required(
                        "rmc-engine was built without the `embeddings-cuda` feature",
                    ));
                }
            }
            EmbeddingRuntime::OpenRouter => EmbeddingGeneratorInner::OpenRouter(Arc::new(
                openrouter::OpenRouterEmbedder::new(&backend)?,
            )),
            EmbeddingRuntime::LocalFastembedOnnxCpu => EmbeddingGeneratorInner::FastembedOnnx(
                Arc::new(fastembed_onnx::FastembedOnnxEmbedder::new_cpu(&backend, owner)?),
            ),
            EmbeddingRuntime::LocalFastembedOnnxMigraphx
            | EmbeddingRuntime::LocalFastembedOnnxDirectml => {
                // Built here if the broker has none, so a broken GPU fails at
                // construction as it always did; otherwise free.
                gpu_broker::ensure_session(&backend, owner)?;
                EmbeddingGeneratorInner::GpuBrokered { owner }
            }
        };
        Ok(Self { inner, backend })
    }

    /// Output vector dimension for the active backend.
    pub fn dimensions(&self) -> usize {
        match &self.inner {
            #[cfg(feature = "embeddings-cuda")]
            EmbeddingGeneratorInner::Qwen3(inner) => inner.dim(),
            EmbeddingGeneratorInner::FastembedOnnx(inner) => inner.dim(),
            EmbeddingGeneratorInner::GpuBrokered { .. } => self.backend.dim(),
            EmbeddingGeneratorInner::OpenRouter(inner) => inner.dim(),
        }
    }

    /// Borrow the active backend configuration.
    pub fn backend(&self) -> &EmbeddingBackend {
        &self.backend
    }

    /// Document-side embedding (raw text, no instruction prefix).
    /// Used by indexer / cache / batcher.
    ///
    /// Every vector passes the non-finite gate before it is returned — see
    /// [`EmbeddingGenerator::guard_finite`].
    pub async fn embed_documents(
        &self,
        texts: Vec<String>,
    ) -> Result<Vec<Embedding>, EmbeddingError> {
        let texts = Arc::new(texts);
        let embeddings = self.embed_raw(EmbedSide::Documents, texts.clone()).await?;
        self.guard_finite(EmbedSide::Documents, texts, embeddings)
            .await
    }

    /// Query-side embedding (Qwen3 instruction prefix applied).
    /// Used by search.
    ///
    /// Gated exactly like the document side: a query vector with a NaN in it
    /// does not fail, it silently loses every comparison, which reads as
    /// "search found nothing" rather than as a fault.
    pub async fn embed_queries(
        &self,
        texts: Vec<String>,
    ) -> Result<Vec<Embedding>, EmbeddingError> {
        let texts = Arc::new(texts);
        let embeddings = self.embed_raw(EmbedSide::Queries, texts.clone()).await?;
        self.guard_finite(EmbedSide::Queries, texts, embeddings).await
    }

    /// Ungated call into the active backend.
    ///
    /// `texts` is shared rather than moved because the non-finite gate may
    /// need the very same inputs again for the one-by-one retry, and cloning
    /// a whole batch of chunk texts on every call just to keep that option
    /// open would be paid on every batch of a full index run.
    async fn embed_raw(
        &self,
        side: EmbedSide,
        texts: Arc<Vec<String>>,
    ) -> Result<Vec<Embedding>, EmbeddingError> {
        // Taken inside each blocking task, for exactly one run of the model;
        // see `gpu_lock` for why another process's run must not overlap it.
        let on_gpu = self.backend.runs_on_gpu();
        match &self.inner {
            #[cfg(feature = "embeddings-cuda")]
            EmbeddingGeneratorInner::Qwen3(inner) => {
                let inner = inner.clone();
                tokio::task::spawn_blocking(move || {
                    let _gpu = on_gpu.then(gpu_lock::GpuLock::acquire).transpose()?;
                    let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
                    match side {
                        EmbedSide::Documents => inner.embed_documents(&refs),
                        EmbedSide::Queries => inner.embed_queries(&refs),
                    }
                })
                .await
                .map_err(|e| EmbeddingError::task_join(e.to_string()))?
            }
            EmbeddingGeneratorInner::FastembedOnnx(inner) => {
                let inner = inner.clone();
                tokio::task::spawn_blocking(move || {
                    let _gpu = on_gpu.then(gpu_lock::GpuLock::acquire).transpose()?;
                    let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
                    match side {
                        EmbedSide::Documents => inner.embed_documents(&refs),
                        EmbedSide::Queries => inner.embed_queries(&refs),
                    }
                })
                .await
                .map_err(|e| EmbeddingError::task_join(e.to_string()))?
            }
            EmbeddingGeneratorInner::GpuBrokered { owner } => {
                let owner = *owner;
                let backend = self.backend.clone();
                tokio::task::spawn_blocking(move || {
                    // The broker orders this process's users (queries
                    // first), the flock orders processes; in that order, so
                    // a query never queues behind another process's lock
                    // while holding nothing.
                    gpu_broker::with_session(&backend, owner, |session| {
                        let _gpu = gpu_lock::GpuLock::acquire()?;
                        let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
                        match side {
                            EmbedSide::Documents => session.embed_documents(&refs),
                            EmbedSide::Queries => session.embed_queries(&refs),
                        }
                    })
                })
                .await
                .map_err(|e| EmbeddingError::task_join(e.to_string()))?
            }
            EmbeddingGeneratorInner::OpenRouter(inner) => {
                // The remote path wants an owned batch, so this is the one
                // branch that pays for the shared ownership above.
                let texts = texts.as_ref().clone();
                match side {
                    EmbedSide::Documents => inner.embed_documents(texts).await,
                    EmbedSide::Queries => inner.embed_queries(texts).await,
                }
            }
        }
    }

    /// Reject a batch that contains a non-finite component, retrying
    /// one-by-one first on the backends where that is a known, measured
    /// workaround.
    ///
    /// # Why this gate exists at all
    ///
    /// The ROCm execution provider returns NaN vectors non-deterministically:
    /// on a 512-chunk sweep, batch 16 corrupted 65% of the rows while batches
    /// 1 and 32 came back clean, the same run repeated on another set
    /// corrupted a *different* set of rows, and the CPU control was clean at
    /// every batch size. The full measurement is in
    /// unpublished notes, section "ROCm EP returns NaN
    /// nondeterministically".
    ///
    /// The failure mode is what makes a gate necessary rather than merely
    /// nice: nothing crashes. A NaN vector loses every cosine comparison, so
    /// the affected chunk simply stops being findable, and from the caller's
    /// side that is indistinguishable from a weak model. An index can rot
    /// this way for weeks without a single error in the log — which is
    /// exactly the shape of failure background sync would otherwise automate.
    ///
    /// # Why a retry, and only here
    ///
    /// What breaks is the POSITION in the batch, not the text: both victims
    /// of the original measurement embedded cleanly on their own, and batch
    /// size 1 never reproduced the fault. Recomputing the batch one item at a
    /// time is therefore the documented workaround, and it costs nothing —
    /// the same sweep measured 17.2 chunk/s at batch 1 against 18.9 at batch
    /// 32. It is applied only to the local ONNX GPU runtimes, because only
    /// there is the fault known to be a batching artifact: a non-finite
    /// vector from the CPU or the remote backend is a real defect, and
    /// recomputing it would just launder a broken result into the index.
    async fn guard_finite(
        &self,
        side: EmbedSide,
        texts: Arc<Vec<String>>,
        embeddings: Vec<Embedding>,
    ) -> Result<Vec<Embedding>, EmbeddingError> {
        let Some(found) = first_non_finite(&embeddings) else {
            return Ok(embeddings);
        };

        if !retries_non_finite(self.backend.runtime) {
            return Err(found.into_error(embeddings.len()));
        }

        tracing::warn!(
            "{} returned a non-finite vector for item {} of {} ({} = {}); recomputing this batch one item at a time",
            self.backend.profile.name(),
            found.index,
            embeddings.len(),
            found.component,
            found.value,
        );

        let mut recomputed = Vec::with_capacity(texts.len());
        for text in texts.iter() {
            let single = Arc::new(vec![text.clone()]);
            let one = self.embed_raw(side, single).await?;

            // Still non-finite with nothing else in the batch: the
            // workaround did not hold, and this is the point where the run
            // must stop rather than write the vector.
            if let Some(found) = first_non_finite(&one) {
                return Err(found.into_error(embeddings.len()));
            }

            recomputed.extend(one);
        }

        Ok(recomputed)
    }

    /// Embed a slice of code chunks for the index.
    ///
    /// Wraps `embed_documents` over each chunk's
    /// `format_for_embedding()` output.
    pub async fn embed_chunks(
        &self,
        chunks: &[CodeChunk],
    ) -> Result<Vec<ChunkWithEmbedding>, EmbeddingError> {
        let formatted: Vec<String> =
            chunks.iter().map(|c| c.format_for_embedding()).collect();
        let embeddings = self.embed_documents(formatted).await?;
        let results: Vec<ChunkWithEmbedding> = chunks
            .iter()
            .zip(embeddings.into_iter())
            .map(|(chunk, embedding)| ChunkWithEmbedding {
                chunk_id: chunk.id,
                embedding,
            })
            .collect();
        Ok(results)
    }
}

/// Embedding pipeline with batch processing and progress reporting.
pub(crate) struct EmbeddingPipeline {
    generator: EmbeddingGenerator,
    batch_size: usize,
}

impl EmbeddingPipeline {
    /// Create a new embedding pipeline.
    pub fn new(generator: EmbeddingGenerator) -> Self {
        Self {
            generator,
            // Starting point for Qwen3-0.6B; calibrate during smoke test.
            batch_size: 32,
        }
    }

    /// Create with a custom batch size.
    pub fn with_batch_size(generator: EmbeddingGenerator, batch_size: usize) -> Self {
        Self {
            generator,
            batch_size,
        }
    }

    /// Process chunks with a progress callback.
    ///
    /// The callback receives `(current, total)` after each batch.
    pub async fn process_chunks<F>(
        &self,
        chunks: Vec<CodeChunk>,
        mut progress: F,
    ) -> Result<Vec<ChunkWithEmbedding>, EmbeddingError>
    where
        F: FnMut(usize, usize),
    {
        let total = chunks.len();
        let mut results = Vec::new();

        for (batch_idx, batch) in chunks.chunks(self.batch_size).enumerate() {
            let batch_results = self.generator.embed_chunks(batch).await?;
            results.extend(batch_results);

            let processed = (batch_idx + 1) * self.batch_size;
            progress(processed.min(total), total);
        }

        Ok(results)
    }

    /// Output vector dimension for the active backend.
    pub fn dimensions(&self) -> usize {
        self.generator.dimensions()
    }
}

#[cfg(test)]
mod non_finite_gate_tests {
    use super::*;

    #[test]
    fn clean_batch_has_no_hit() {
        let batch = vec![vec![0.1, -0.2, 0.3], vec![0.0, 1.0, -1.0]];

        assert_eq!(first_non_finite(&batch), None);
    }

    #[test]
    fn reports_first_nan_with_its_position() {
        let batch = vec![vec![0.1, 0.2], vec![0.3, f32::NAN], vec![f32::NAN, 0.4]];

        let hit = first_non_finite(&batch).expect("NaN is not finite");

        assert_eq!(hit.index, 1);
        assert_eq!(hit.component, 1);
        assert!(hit.value.is_nan());
    }

    /// Infinity is rejected too: it is not what the ROCm measurement produced,
    /// but it is equally unusable downstream, and a gate that let it through
    /// would be a gate against one spelling of the fault rather than against
    /// the fault.
    #[test]
    fn infinity_counts_as_non_finite() {
        let batch = vec![vec![0.1, f32::INFINITY], vec![0.2, 0.3]];

        let hit = first_non_finite(&batch).expect("infinity is not finite");

        assert_eq!((hit.index, hit.component), (0, 1));
        assert_eq!(hit.value, f32::INFINITY);
    }

    #[test]
    fn empty_batch_and_empty_vectors_are_clean() {
        assert_eq!(first_non_finite(&[]), None);
        assert_eq!(first_non_finite(&[vec![]]), None);
    }

    #[test]
    fn error_carries_the_batch_length_the_caller_saw() {
        let hit = NonFiniteHit {
            index: 3,
            component: 7,
            value: f32::NAN,
        };

        let message = hit.into_error(32).to_string();

        assert!(message.contains("item 3 of 32"), "{message}");
        assert!(message.contains("component 7"), "{message}");
    }

    /// The retry is a workaround for a measured batching artifact of the
    /// local ONNX GPU execution providers. Everywhere else a non-finite
    /// vector is a real defect, and recomputing it would only hide it.
    #[test]
    fn retry_is_limited_to_local_onnx_gpu_runtimes() {
        assert!(retries_non_finite(
            EmbeddingRuntime::LocalFastembedOnnxMigraphx
        ));
        assert!(retries_non_finite(
            EmbeddingRuntime::LocalFastembedOnnxDirectml
        ));

        assert!(!retries_non_finite(EmbeddingRuntime::LocalFastembedOnnxCpu));
        assert!(!retries_non_finite(EmbeddingRuntime::LocalQwen3CandleCuda));
        assert!(!retries_non_finite(EmbeddingRuntime::OpenRouter));
    }
}
