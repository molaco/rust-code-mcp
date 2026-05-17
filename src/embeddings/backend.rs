//! Embedding backend configuration.
//!
//! Defines embedding profiles, runtimes, model specs, dimensions, and stable
//! identity strings used in cache paths and `EMBEDDER_VERSION`.

use super::error::EmbeddingError;

pub(crate) const QWEN3_CODE_QUERY_PREFIX: &str =
    "Instruct: Given a code search query, retrieve relevant code\nQuery: ";
pub(crate) const BGE_SEARCH_QUERY_PREFIX: &str =
    "Represent this sentence for searching relevant passages: ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EmbeddingBackend {
    pub profile: EmbeddingProfile,
    pub runtime: EmbeddingRuntime,
    pub model: EmbeddingModelSpec,
    pub max_len: usize,
    /// Off by default. Set only for CI/benchmark runs. Enabling this
    /// emits a warn! on every construction.
    pub force_cpu: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EmbeddingProfile {
    LocalGpuSmall,
    LocalQwen3_4B,
    LocalQwen3_8B,
    LocalCpuSmall,
    OpenRouterQwen3_8B,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EmbeddingRuntime {
    LocalQwen3CandleCuda,
    LocalFastembedOnnxCpu,
    OpenRouter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EmbeddingModelSpec {
    Qwen3Embedding0_6B,
    Qwen3Embedding4B,
    Qwen3Embedding8B,
    BgeSmallEnV15Q,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueryFormatting {
    Qwen3CodeInstruction,
    BgeSearchInstruction,
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

impl EmbeddingProfile {
    pub fn name(self) -> &'static str {
        match self {
            Self::LocalGpuSmall => "local-gpu-small",
            Self::LocalQwen3_4B => "local-qwen3-4b",
            Self::LocalQwen3_8B => "local-qwen3-8b",
            Self::LocalCpuSmall => "local-cpu-small",
            Self::OpenRouterQwen3_8B => "openrouter-qwen3-8b",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "local-gpu-small" | "qwen3-local-gpu-small" => Ok(Self::LocalGpuSmall),
            "local-qwen3-4b" => Ok(Self::LocalQwen3_4B),
            "local-qwen3-8b" => Ok(Self::LocalQwen3_8B),
            "local-cpu-small" | "bge-small-cpu" => Ok(Self::LocalCpuSmall),
            "openrouter-qwen3-8b" | "qwen3-8b-openrouter" => {
                Ok(Self::OpenRouterQwen3_8B)
            }
            other => Err(format!(
                "unknown embedding profile: {other}; expected one of: {}",
                Self::accepted_names()
            )),
        }
    }

    pub fn accepted_names() -> &'static str {
        "local-gpu-small, local-cpu-small, openrouter-qwen3-8b, local-qwen3-4b, local-qwen3-8b"
    }

    pub fn runtime(self) -> EmbeddingRuntime {
        match self {
            Self::LocalGpuSmall | Self::LocalQwen3_4B | Self::LocalQwen3_8B => {
                EmbeddingRuntime::LocalQwen3CandleCuda
            }
            Self::LocalCpuSmall => EmbeddingRuntime::LocalFastembedOnnxCpu,
            Self::OpenRouterQwen3_8B => EmbeddingRuntime::OpenRouter,
        }
    }

    pub fn model(self) -> EmbeddingModelSpec {
        match self {
            Self::LocalGpuSmall => EmbeddingModelSpec::Qwen3Embedding0_6B,
            Self::LocalQwen3_4B => EmbeddingModelSpec::Qwen3Embedding4B,
            Self::LocalQwen3_8B | Self::OpenRouterQwen3_8B => {
                EmbeddingModelSpec::Qwen3Embedding8B
            }
            Self::LocalCpuSmall => EmbeddingModelSpec::BgeSmallEnV15Q,
        }
    }

    pub fn default_max_len(self) -> usize {
        match self {
            Self::LocalGpuSmall | Self::LocalQwen3_4B | Self::LocalQwen3_8B => 1024,
            Self::LocalCpuSmall => 512,
            Self::OpenRouterQwen3_8B => 32_768,
        }
    }

    pub fn default_chunk_target_tokens(self) -> usize {
        match self {
            Self::LocalCpuSmall => 384,
            Self::LocalGpuSmall
            | Self::LocalQwen3_4B
            | Self::LocalQwen3_8B
            | Self::OpenRouterQwen3_8B => 768,
        }
    }

    pub fn default_chunk_hard_max_tokens(self) -> usize {
        match self {
            Self::LocalCpuSmall => 512,
            Self::LocalGpuSmall
            | Self::LocalQwen3_4B
            | Self::LocalQwen3_8B
            | Self::OpenRouterQwen3_8B => 1024,
        }
    }

    pub fn query_formatting(self) -> QueryFormatting {
        match self {
            Self::LocalCpuSmall => QueryFormatting::BgeSearchInstruction,
            Self::LocalGpuSmall
            | Self::LocalQwen3_4B
            | Self::LocalQwen3_8B
            | Self::OpenRouterQwen3_8B => QueryFormatting::Qwen3CodeInstruction,
        }
    }
}

impl EmbeddingModelSpec {
    pub fn dim(self) -> usize {
        match self {
            Self::Qwen3Embedding0_6B => 1024,
            Self::Qwen3Embedding4B => 2560,
            Self::Qwen3Embedding8B => 4096,
            Self::BgeSmallEnV15Q => 384,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Qwen3Embedding0_6B => "Qwen3-Embedding-0.6B",
            Self::Qwen3Embedding4B => "Qwen3-Embedding-4B",
            Self::Qwen3Embedding8B => "Qwen3-Embedding-8B",
            Self::BgeSmallEnV15Q => "BGESmallENV15Q",
        }
    }

    pub fn provider_model_id(self) -> &'static str {
        match self {
            Self::Qwen3Embedding0_6B => "Qwen/Qwen3-Embedding-0.6B",
            Self::Qwen3Embedding4B => "Qwen/Qwen3-Embedding-4B",
            Self::Qwen3Embedding8B => "Qwen/Qwen3-Embedding-8B",
            Self::BgeSmallEnV15Q => "Qdrant/bge-small-en-v1.5-onnx-Q",
        }
    }

    pub fn openrouter_model_id(self) -> Option<&'static str> {
        match self {
            Self::Qwen3Embedding8B => Some("qwen/qwen3-embedding-8b"),
            _ => None,
        }
    }

    pub fn qwen3_variant(self) -> Option<Qwen3Variant> {
        match self {
            Self::Qwen3Embedding0_6B => Some(Qwen3Variant::Embedding0_6B),
            Self::Qwen3Embedding4B => Some(Qwen3Variant::Embedding4B),
            Self::Qwen3Embedding8B => Some(Qwen3Variant::Embedding8B),
            Self::BgeSmallEnV15Q => None,
        }
    }
}

impl QueryFormatting {
    pub fn format_query(self, text: &str) -> String {
        match self {
            Self::Qwen3CodeInstruction => format!("{QWEN3_CODE_QUERY_PREFIX}{text}"),
            Self::BgeSearchInstruction => format!("{BGE_SEARCH_QUERY_PREFIX}{text}"),
        }
    }
}

impl Default for EmbeddingBackend {
    fn default() -> Self {
        Self::from_profile(EmbeddingProfile::LocalGpuSmall)
    }
}

impl EmbeddingBackend {
    pub fn from_profile(profile: EmbeddingProfile) -> Self {
        Self {
            profile,
            runtime: profile.runtime(),
            model: profile.model(),
            max_len: profile.default_max_len(),
            force_cpu: false,
        }
    }

    pub fn from_profile_name(name: &str) -> Result<Self, EmbeddingError> {
        let profile = EmbeddingProfile::parse(name).map_err(EmbeddingError::model_init)?;
        Ok(Self::from_profile(profile))
    }

    pub fn from_qwen3_variant(variant: Qwen3Variant) -> Self {
        let profile = match variant {
            Qwen3Variant::Embedding0_6B => EmbeddingProfile::LocalGpuSmall,
            Qwen3Variant::Embedding4B => EmbeddingProfile::LocalQwen3_4B,
            Qwen3Variant::Embedding8B => EmbeddingProfile::LocalQwen3_8B,
        };
        Self::from_profile(profile)
    }

    pub fn dim(&self) -> usize {
        self.model.dim()
    }

    pub fn qwen3_variant(&self) -> Option<Qwen3Variant> {
        self.model.qwen3_variant()
    }

    pub fn require_qwen3_variant(&self) -> Result<Qwen3Variant, EmbeddingError> {
        self.qwen3_variant().ok_or_else(|| {
            EmbeddingError::model_init(format!(
                "embedding profile `{}` does not use the local Qwen3 runtime",
                self.profile.name()
            ))
        })
    }

    pub fn query_formatting(&self) -> QueryFormatting {
        self.profile.query_formatting()
    }

    pub fn format_query(&self, text: &str) -> String {
        self.query_formatting().format_query(text)
    }

    /// Stable string used in cache paths and EMBEDDER_VERSION.
    ///
    /// The default local Qwen3-Embedding-0.6B identity intentionally keeps the
    /// existing `v2` string so current indexes remain compatible.
    pub fn identity(&self) -> String {
        match self.runtime {
            EmbeddingRuntime::LocalQwen3CandleCuda => format!(
                "fastembed-candle:{}:dim{}:max{}:v2",
                self.model.display_name(),
                self.dim(),
                self.max_len,
            ),
            EmbeddingRuntime::LocalFastembedOnnxCpu => format!(
                "fastembed-onnx-cpu:{}:dim{}:max{}:v1",
                self.model.display_name(),
                self.dim(),
                self.max_len,
            ),
            EmbeddingRuntime::OpenRouter => format!(
                "openrouter:{}:dim{}:max{}:v1",
                self.model
                    .openrouter_model_id()
                    .unwrap_or(self.model.provider_model_id()),
                self.dim(),
                self.max_len,
            ),
        }
    }

    /// Parse an `identity()` string back into an `EmbeddingBackend`.
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

        let mut backend = match parts[0] {
            "fastembed-candle" => {
                if parts[4] != "v2" {
                    return Err(EmbeddingError::invalid_identity(format!(
                        "unsupported identity schema version `{}` in `{}`",
                        parts[4], s
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
                Self::from_qwen3_variant(variant)
            }
            "fastembed-onnx-cpu" => {
                if parts[4] != "v1" {
                    return Err(EmbeddingError::invalid_identity(format!(
                        "unsupported identity schema version `{}` in `{}`",
                        parts[4], s
                    )));
                }
                match parts[1] {
                    "BGESmallENV15Q" => Self::from_profile(EmbeddingProfile::LocalCpuSmall),
                    other => {
                        return Err(EmbeddingError::invalid_identity(format!(
                            "unknown ONNX model `{}` in `{}`",
                            other, s
                        )));
                    }
                }
            }
            "openrouter" => {
                if parts[4] != "v1" {
                    return Err(EmbeddingError::invalid_identity(format!(
                        "unsupported identity schema version `{}` in `{}`",
                        parts[4], s
                    )));
                }
                match parts[1] {
                    "qwen/qwen3-embedding-8b" => {
                        Self::from_profile(EmbeddingProfile::OpenRouterQwen3_8B)
                    }
                    other => {
                        return Err(EmbeddingError::invalid_identity(format!(
                            "unknown OpenRouter model `{}` in `{}`",
                            other, s
                        )));
                    }
                }
            }
            other => {
                return Err(EmbeddingError::invalid_identity(format!(
                    "unexpected backend prefix `{}` in `{}`",
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
        if dim != backend.dim() {
            return Err(EmbeddingError::invalid_identity(format!(
                "dim `{}` does not match model {:?} (expected {}) in `{}`",
                dim,
                backend.model,
                backend.dim(),
                s
            )));
        }

        let max_str = parts[3].strip_prefix("max").ok_or_else(|| {
            EmbeddingError::invalid_identity(format!("missing `max` prefix in `{}`", s))
        })?;
        backend.max_len = max_str.parse().map_err(|e| {
            EmbeddingError::invalid_identity(format!(
                "failed to parse max_len `{}` in `{}`: {}",
                max_str, s, e
            ))
        })?;

        Ok(backend)
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
    fn default_backend_identity_matches_existing_qwen3_identity() {
        assert_eq!(
            EmbeddingBackend::default().identity(),
            "fastembed-candle:Qwen3-Embedding-0.6B:dim1024:max1024:v2"
        );
    }

    #[test]
    fn profile_dimensions_match_expected_values() {
        assert_eq!(EmbeddingBackend::from_profile(EmbeddingProfile::LocalCpuSmall).dim(), 384);
        assert_eq!(
            EmbeddingBackend::from_profile(EmbeddingProfile::OpenRouterQwen3_8B).dim(),
            4096
        );
        assert_eq!(Qwen3Variant::Embedding4B.dim(), 2560);
        assert_eq!(Qwen3Variant::Embedding8B.dim(), 4096);
    }

    #[test]
    fn identities_are_unique_by_profile() {
        let profiles = [
            EmbeddingProfile::LocalGpuSmall,
            EmbeddingProfile::LocalQwen3_4B,
            EmbeddingProfile::LocalQwen3_8B,
            EmbeddingProfile::LocalCpuSmall,
            EmbeddingProfile::OpenRouterQwen3_8B,
        ];
        let mut identities = std::collections::HashSet::new();

        for profile in profiles {
            assert!(identities.insert(EmbeddingBackend::from_profile(profile).identity()));
        }
    }

    #[test]
    fn profile_parse_accepts_explicit_profiles() {
        assert_eq!(
            EmbeddingProfile::parse("local-gpu-small").unwrap(),
            EmbeddingProfile::LocalGpuSmall
        );
        assert_eq!(
            EmbeddingProfile::parse("local-cpu-small").unwrap(),
            EmbeddingProfile::LocalCpuSmall
        );
        assert_eq!(
            EmbeddingProfile::parse("openrouter-qwen3-8b").unwrap(),
            EmbeddingProfile::OpenRouterQwen3_8B
        );
    }

    #[test]
    fn query_formatting_is_profile_aware() {
        assert_eq!(
            EmbeddingBackend::from_profile(EmbeddingProfile::LocalGpuSmall)
                .format_query("find parser"),
            "Instruct: Given a code search query, retrieve relevant code\nQuery: find parser"
        );
        assert_eq!(
            EmbeddingBackend::from_profile(EmbeddingProfile::LocalCpuSmall)
                .format_query("find parser"),
            "Represent this sentence for searching relevant passages: find parser"
        );
    }

    #[test]
    fn from_identity_roundtrip_default() {
        let original = EmbeddingBackend::default();
        let parsed = EmbeddingBackend::from_identity(&original.identity()).unwrap();
        assert_eq!(parsed.profile, original.profile);
        assert_eq!(parsed.max_len, original.max_len);
    }

    #[test]
    fn from_identity_roundtrip_4b() {
        let original = EmbeddingBackend::from_qwen3_variant(Qwen3Variant::Embedding4B);
        let parsed = EmbeddingBackend::from_identity(&original.identity()).unwrap();
        assert_eq!(parsed.profile, original.profile);
        assert_eq!(parsed.max_len, original.max_len);
    }

    #[test]
    fn from_identity_roundtrip_cpu_profile() {
        let original = EmbeddingBackend::from_profile(EmbeddingProfile::LocalCpuSmall);
        let parsed = EmbeddingBackend::from_identity(&original.identity()).unwrap();
        assert_eq!(parsed.profile, original.profile);
        assert_eq!(parsed.max_len, original.max_len);
    }

    #[test]
    fn from_identity_roundtrip_openrouter_profile() {
        let original = EmbeddingBackend::from_profile(EmbeddingProfile::OpenRouterQwen3_8B);
        let parsed = EmbeddingBackend::from_identity(&original.identity()).unwrap();
        assert_eq!(parsed.profile, original.profile);
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
