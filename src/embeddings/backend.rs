//! Embedding backend configuration
//!
//! Defines the Qwen3 embedding backend variants, their dimensions, and a
//! stable identity string used in cache paths and `EMBEDDER_VERSION`.

use super::error::EmbeddingError;

#[derive(Debug, Clone, Copy)]
pub struct EmbeddingBackend {
    pub variant: Qwen3Variant,
    pub max_len: usize,
    /// Off by default. Set only for CI/benchmark runs. Enabling this
    /// emits a warn! on every construction.
    pub force_cpu: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Qwen3Variant {
    Embedding0_6B,
    Embedding4B,
    Embedding8B,
}

impl Qwen3Variant {
    pub fn dim(self) -> usize {
        match self {
            Self::Embedding0_6B => 1024,
            Self::Embedding4B => 2560,
            Self::Embedding8B => 4096,
        }
    }

    pub fn hf_model_id(self) -> &'static str {
        match self {
            Self::Embedding0_6B => "Qwen/Qwen3-Embedding-0.6B",
            Self::Embedding4B => "Qwen/Qwen3-Embedding-4B",
            Self::Embedding8B => "Qwen/Qwen3-Embedding-8B",
        }
    }
}

impl Default for EmbeddingBackend {
    fn default() -> Self {
        Self {
            variant: Qwen3Variant::Embedding0_6B,
            // 1024 keeps attention memory bounded: at max_len=2048,
            // Qwen3-0.6B's per-layer attention matrix is
            // [batch, heads, seq, seq] x 4 bytes -> ~4 GB for one
            // layer at batch=8, which OOMs on real-corpus chunks
            // even on a 24 GB card. 1024 cuts that 4x and still
            // covers virtually all function-sized chunks after the
            // contextual-retrieval header.
            max_len: 1024,
            force_cpu: false,
        }
    }
}

impl EmbeddingBackend {
    pub fn dim(&self) -> usize {
        self.variant.dim()
    }

    /// Stable string used in cache paths and EMBEDDER_VERSION.
    /// Example: "fastembed-candle:Qwen3-Embedding-0.6B:dim1024:max1024:v2"
    pub fn identity(&self) -> String {
        format!(
            "fastembed-candle:{}:dim{}:max{}:v2",
            self.variant.hf_model_id().rsplit('/').next().unwrap_or("unknown"),
            self.dim(),
            self.max_len,
        )
    }

    /// Parse an `identity()` string back into an `EmbeddingBackend`.
    ///
    /// Accepts the exact format produced by `identity()`:
    /// `fastembed-candle:Qwen3-Embedding-<X>:dim<Y>:max<Z>:v2`. The
    /// `dim` portion is informational — the variant fully determines
    /// the dimension — but must agree with the variant. `force_cpu` is
    /// not encoded in the identity and defaults to `false`.
    ///
    /// Used to reconcile the embedder recorded in a vector store's
    /// `metadata.json` with the embedder a search-time caller wants.
    pub fn from_identity(s: &str) -> Result<Self, EmbeddingError> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 5 {
            return Err(EmbeddingError::invalid_identity(format!(
                "expected 5 colon-separated fields, got {}: `{}`",
                parts.len(),
                s
            )));
        }
        if parts[0] != "fastembed-candle" {
            return Err(EmbeddingError::invalid_identity(format!(
                "unexpected backend prefix `{}` in `{}`",
                parts[0], s
            )));
        }
        let variant = match parts[1] {
            "Qwen3-Embedding-0.6B" => Qwen3Variant::Embedding0_6B,
            "Qwen3-Embedding-4B" => Qwen3Variant::Embedding4B,
            "Qwen3-Embedding-8B" => Qwen3Variant::Embedding8B,
            other => {
                return Err(EmbeddingError::invalid_identity(format!(
                    "unknown Qwen3 variant `{}` in `{}`",
                    other, s
                )));
            }
        };
        let dim_str = parts[2].strip_prefix("dim").ok_or_else(|| {
            EmbeddingError::invalid_identity(format!("missing `dim` prefix in `{}`", s))
        })?;
        let dim: usize = dim_str.parse().map_err(|e| {
            EmbeddingError::invalid_identity(format!(
                "failed to parse dim `{}` in `{}`: {}",
                dim_str, s, e
            ))
        })?;
        if dim != variant.dim() {
            return Err(EmbeddingError::invalid_identity(format!(
                "dim `{}` does not match variant {:?} (expected {}) in `{}`",
                dim,
                variant,
                variant.dim(),
                s
            )));
        }
        let max_str = parts[3].strip_prefix("max").ok_or_else(|| {
            EmbeddingError::invalid_identity(format!("missing `max` prefix in `{}`", s))
        })?;
        let max_len: usize = max_str.parse().map_err(|e| {
            EmbeddingError::invalid_identity(format!(
                "failed to parse max_len `{}` in `{}`: {}",
                max_str, s, e
            ))
        })?;
        if parts[4] != "v2" {
            return Err(EmbeddingError::invalid_identity(format!(
                "unsupported identity schema version `{}` in `{}`",
                parts[4], s
            )));
        }
        Ok(Self {
            variant,
            max_len,
            force_cpu: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_backend_dim_is_1024() {
        assert_eq!(EmbeddingBackend::default().dim(), 1024);
    }

    #[test]
    fn default_backend_identity_matches_expected() {
        assert_eq!(
            EmbeddingBackend::default().identity(),
            "fastembed-candle:Qwen3-Embedding-0.6B:dim1024:max1024:v2"
        );
    }

    #[test]
    fn variant_4b_dim_is_2560() {
        assert_eq!(Qwen3Variant::Embedding4B.dim(), 2560);
    }

    #[test]
    fn variant_8b_dim_is_4096() {
        assert_eq!(Qwen3Variant::Embedding8B.dim(), 4096);
    }

    #[test]
    fn from_identity_roundtrip_default() {
        let original = EmbeddingBackend::default();
        let parsed = EmbeddingBackend::from_identity(&original.identity()).unwrap();
        assert_eq!(parsed.variant, original.variant);
        assert_eq!(parsed.max_len, original.max_len);
    }

    #[test]
    fn from_identity_roundtrip_4b() {
        let original = EmbeddingBackend {
            variant: Qwen3Variant::Embedding4B,
            max_len: 2048,
            force_cpu: false,
        };
        let parsed = EmbeddingBackend::from_identity(&original.identity()).unwrap();
        assert_eq!(parsed.variant, original.variant);
        assert_eq!(parsed.max_len, original.max_len);
    }

    #[test]
    fn from_identity_rejects_garbage() {
        assert!(EmbeddingBackend::from_identity("garbage").is_err());
        assert!(EmbeddingBackend::from_identity(
            "fastembed-candle:Qwen3-Embedding-0.6B:dim999:max2048:v2"
        )
        .is_err());
        assert!(EmbeddingBackend::from_identity(
            "fastembed-candle:Qwen3-Embedding-0.6B:dim1024:max1024:v1"
        )
        .is_err());
    }
}
