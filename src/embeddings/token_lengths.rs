//! Token-length measurement for Qwen3 embedding inputs.

use hf_hub::api::sync::ApiBuilder;
use tokenizers::Tokenizer;

use crate::embeddings::{EmbeddingBackend, EmbeddingError};

/// Raw and model-capped token length for one embedding input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddingTextLen {
    pub raw_tokens: usize,
    pub capped_tokens: usize,
}

/// Token counter using the same Qwen3 tokenizer path as fastembed.
pub struct EmbeddingTokenCounter {
    tokenizer: Tokenizer,
    max_len: usize,
}

impl EmbeddingTokenCounter {
    /// Load the tokenizer for the active backend.
    pub fn from_backend(backend: &EmbeddingBackend) -> Result<Self, EmbeddingError> {
        let api = ApiBuilder::new()
            .with_progress(false)
            .build()
            .map_err(|e| EmbeddingError::model_init(e.to_string()))?;
        let repo = api.model(backend.model.provider_model_id().to_string());
        let tokenizer_path = repo
            .get("tokenizer.json")
            .map_err(|e| EmbeddingError::model_init(e.to_string()))?;
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| EmbeddingError::model_init(e.to_string()))?;

        Ok(Self {
            tokenizer,
            max_len: backend.max_len,
        })
    }

    pub fn max_len(&self) -> usize {
        self.max_len
    }

    /// Count one text using the same special-token behavior as fastembed.
    pub fn count(&self, text: &str) -> Result<EmbeddingTextLen, EmbeddingError> {
        let encoded = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| EmbeddingError::embed_failed(e.to_string()))?;
        Ok(self.len_from_raw(encoded.get_ids().len()))
    }

    /// Count a batch of texts using the same special-token behavior as fastembed.
    pub fn count_batch<S: AsRef<str>>(
        &self,
        texts: &[S],
    ) -> Result<Vec<EmbeddingTextLen>, EmbeddingError> {
        let refs: Vec<&str> = texts.iter().map(|text| text.as_ref()).collect();
        let encodings = self
            .tokenizer
            .encode_batch(refs, true)
            .map_err(|e| EmbeddingError::embed_failed(e.to_string()))?;
        Ok(encodings
            .iter()
            .map(|encoding| self.len_from_raw(encoding.get_ids().len()))
            .collect())
    }

    fn len_from_raw(&self, raw_tokens: usize) -> EmbeddingTextLen {
        EmbeddingTextLen {
            raw_tokens,
            capped_tokens: raw_tokens.min(self.max_len),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_lengths_at_backend_max_len() {
        let counter = EmbeddingTokenCounter {
            tokenizer: Tokenizer::new(tokenizers::models::wordlevel::WordLevel::default()),
            max_len: 3,
        };

        assert_eq!(
            counter.len_from_raw(5),
            EmbeddingTextLen {
                raw_tokens: 5,
                capped_tokens: 3,
            }
        );
        assert_eq!(
            counter.len_from_raw(2),
            EmbeddingTextLen {
                raw_tokens: 2,
                capped_tokens: 2,
            }
        );
    }
}
