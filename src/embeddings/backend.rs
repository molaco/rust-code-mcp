//! Embedding backend configuration
//!
//! Defines the Qwen3 embedding backend variants, their dimensions, and a
//! stable identity string used in cache paths and `EMBEDDER_VERSION`.

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
            max_len: 2048,
            force_cpu: false,
        }
    }
}

impl EmbeddingBackend {
    pub fn dim(&self) -> usize {
        self.variant.dim()
    }

    /// Stable string used in cache paths and EMBEDDER_VERSION.
    /// Example: "fastembed-candle:Qwen3-Embedding-0.6B:dim1024:max2048:v1"
    pub fn identity(&self) -> String {
        format!(
            "fastembed-candle:{}:dim{}:max{}:v1",
            self.variant.hf_model_id().rsplit('/').next().unwrap_or("unknown"),
            self.dim(),
            self.max_len,
        )
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
            "fastembed-candle:Qwen3-Embedding-0.6B:dim1024:max2048:v1"
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
}
