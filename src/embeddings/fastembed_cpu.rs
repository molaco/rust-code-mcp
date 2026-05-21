//! CPU text embedding backend backed by fastembed's ONNX path.

use crate::embeddings::backend::{EmbeddingBackend, EmbeddingRuntime};
use crate::embeddings::profile::FastembedCpuModel;
use crate::embeddings::{Embedding, EmbeddingError};
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use std::sync::Mutex;

pub(super) struct FastembedCpuEmbedder {
    inner: Mutex<TextEmbedding>,
    backend: EmbeddingBackend,
    dim: usize,
}

impl FastembedCpuEmbedder {
    pub(super) fn new(backend: &EmbeddingBackend) -> Result<Self, EmbeddingError> {
        if backend.runtime != EmbeddingRuntime::LocalFastembedOnnxCpu {
            return Err(EmbeddingError::model_init(format!(
                "embedding profile `{}` is not a fastembed ONNX CPU profile",
                backend.profile.name()
            )));
        }
        let model = backend.require_fastembed_cpu_model()?;

        tracing::info!(
            target: "embeddings::fastembed_cpu",
            profile = backend.profile.name(),
            model = model.display_name(),
            max_len = backend.max_len,
            "loading fastembed CPU model"
        );

        let options = TextInitOptions::new(to_fastembed_model(model))
            .with_max_length(backend.max_len)
            .with_show_download_progress(false);
        let inner = TextEmbedding::try_new(options)
            .map_err(|e| EmbeddingError::model_init(e.to_string()))?;

        Ok(Self {
            inner: Mutex::new(inner),
            backend: backend.clone(),
            dim: backend.dim(),
        })
    }

    pub(super) fn dim(&self) -> usize {
        self.dim
    }

    pub(super) fn embed_documents(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Embedding>, EmbeddingError> {
        let mut model = self.inner.lock().unwrap();
        model
            .embed(texts, None)
            .map_err(|e| EmbeddingError::embed_failed(e.to_string()))
    }

    pub(super) fn embed_queries(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Embedding>, EmbeddingError> {
        let prefixed: Vec<String> = texts
            .iter()
            .map(|text| self.backend.format_query(text))
            .collect();
        let refs: Vec<&str> = prefixed.iter().map(String::as_str).collect();
        self.embed_documents(&refs)
    }
}

fn to_fastembed_model(model: FastembedCpuModel) -> EmbeddingModel {
    match model {
        FastembedCpuModel::BgeSmallEnV15Q => EmbeddingModel::BGESmallENV15Q,
    }
}
