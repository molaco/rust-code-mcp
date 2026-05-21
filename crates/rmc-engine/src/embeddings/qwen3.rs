//! Qwen3 embedder backed by fastembed's Candle integration.
//!
//! Constructed by EmbeddingGenerator in Step 4. This module is the
//! sole owner of the candle-core <-> fastembed bridge.

use candle_core::{DType, Device};
use fastembed::Qwen3TextEmbedding;
use std::sync::Mutex;

use crate::embeddings::Embedding;
use crate::embeddings::backend::EmbeddingBackend;
use crate::embeddings::error::EmbeddingError;

pub(super) struct Qwen3Embedder {
    inner: Mutex<Qwen3TextEmbedding>,
    backend: EmbeddingBackend,
    dim: usize,
}

impl Qwen3Embedder {
    pub(super) fn new(backend: &EmbeddingBackend) -> Result<Self, EmbeddingError> {
        let variant = backend.require_qwen3_variant()?;
        let device = if backend.force_cpu {
            tracing::warn!(
                "Qwen3Embedder: force_cpu=true; embedding will run on CPU. \
                 This is supported only for CI/benchmarks — recall and \
                 throughput will be poor."
            );
            Device::Cpu
        } else {
            build_cuda_device()?
        };

        tracing::info!(
            target: "embeddings::qwen3",
            "=== Qwen3 INITIALIZATION ===",
        );
        // F16 halves model weights AND activations vs F32. Upstream
        // fastembed fixed Qwen3 F16 dtype mismatches in commit b39d84b
        // (landed pre-5.13.4). F32 OOMed on real-corpus indexing even
        // at batch=8 / max_len=1024 because attention scores are
        // O(seq^2) and Qwen3-0.6B's ~28 layers stack up. Revisit if
        // we observe NaN / quality regressions on the search side.
        let dtype = DType::F16;

        tracing::info!(
            target: "embeddings::qwen3",
            profile = backend.profile.name(),
            model = backend.model_display_name(),
            model_id = variant.hf_model_id(),
            max_len = backend.max_len,
            ?dtype,
            device = ?device,
            "loading Qwen3 model"
        );

        let inner = Qwen3TextEmbedding::from_hf(
            variant.hf_model_id(),
            &device,
            dtype,
            backend.max_len,
        )
        .map_err(|e| EmbeddingError::model_init(e.to_string()))?;

        let dim = backend.dim();
        tracing::info!(
            target: "embeddings::qwen3",
            "Qwen3Embedder initialized (dim={dim})"
        );

        Ok(Self {
            inner: Mutex::new(inner),
            backend: backend.clone(),
            dim,
        })
    }

    pub(super) fn dim(&self) -> usize {
        self.dim
    }

    pub(super) fn embed_documents(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Embedding>, EmbeddingError> {
        // fastembed's Qwen3TextEmbedding::embed takes &self; the Mutex
        // serializes concurrent calls into the underlying Candle model.
        let model = self.inner.lock().unwrap();
        model
            .embed(texts)
            .map_err(|e| EmbeddingError::embed_failed(e.to_string()))
    }

    pub(super) fn embed_queries(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Embedding>, EmbeddingError> {
        let prefixed: Vec<String> = texts
            .iter()
            .map(|t| self.backend.format_query(t))
            .collect();
        let refs: Vec<&str> = prefixed.iter().map(String::as_str).collect();
        self.embed_documents(&refs)
    }
}

fn build_cuda_device() -> Result<Device, EmbeddingError> {
    // Mirror the spirit of the old ORT CUDA audit but for Candle.
    // We log the env up front so failure modes (missing CUDA_HOME,
    // empty LD_LIBRARY_PATH) are diagnosable from the log alone.
    let cuda_home = std::env::var("CUDA_HOME").ok();
    let cuda_path = std::env::var("CUDA_PATH").ok();
    let ld_library_path_first = std::env::var("LD_LIBRARY_PATH")
        .ok()
        .and_then(|s| s.split(':').next().map(str::to_string));
    tracing::info!(
        target: "embeddings::qwen3",
        ?cuda_home,
        ?cuda_path,
        ?ld_library_path_first,
        "Candle CUDA env probe"
    );

    Device::new_cuda(0).map_err(|e| {
        EmbeddingError::gpu_required(format!(
            "Candle CUDA device construction failed: {e}. \
             Verify CUDA_HOME / CUDA_PATH point at the cudatoolkit and \
             that LD_LIBRARY_PATH includes /run/opengl-driver/lib, \
             cudatoolkit/lib, cuda_cudart/lib, libcublas/lib, cudnn/lib. \
             Build/run via `nix develop ../nix-devshells#cuda-code`."
        ))
    })
}
