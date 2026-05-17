//! Embedding backend configuration.
//!
//! Defines embedding profiles, runtimes, model loader specs, dimensions, and
//! stable identity strings used in cache paths and `EMBEDDER_VERSION`.

use super::error::EmbeddingError;
use std::sync::{Arc, LazyLock};

pub(crate) const QWEN3_CODE_QUERY_PREFIX: &str =
    "Instruct: Given a code search query, retrieve relevant code\nQuery: ";
pub(crate) const BGE_SEARCH_QUERY_PREFIX: &str =
    "Represent this sentence for searching relevant passages: ";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EmbeddingBackend {
    pub profile: EmbeddingProfile,
    pub runtime: EmbeddingRuntime,
    pub max_len: usize,
    /// Off by default. Set only for CI/benchmark runs. Enabling this
    /// emits a warn! on every construction.
    pub force_cpu: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EmbeddingProfile {
    pub name: Arc<str>,
    pub runtime: EmbeddingRuntime,
    pub model_id: Arc<str>,
    pub tokenizer_model_id: Option<Arc<str>>,
    pub dim: usize,
    pub max_len: usize,
    pub query_policy: QueryPolicy,
    pub chunk_target_tokens: usize,
    pub chunk_hard_max_tokens: usize,
    pub local_loader: Option<LocalLoaderSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EmbeddingRuntime {
    LocalQwen3CandleCuda,
    LocalFastembedOnnxCpu,
    OpenRouter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocalLoaderSpec {
    Qwen3(Qwen3Variant),
    FastembedCpu(FastembedCpuModel),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FastembedCpuModel {
    BgeSmallEnV15Q,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum QueryPolicy {
    InstructionPrefix(Arc<str>),
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Qwen3Variant {
    Embedding0_6B,
    Embedding4B,
    Embedding8B,
}

static BUILT_IN_PROFILES: LazyLock<Vec<EmbeddingProfile>> = LazyLock::new(|| {
    vec![
        EmbeddingProfile {
            name: arc("local-gpu-small"),
            runtime: EmbeddingRuntime::LocalQwen3CandleCuda,
            model_id: arc("Qwen/Qwen3-Embedding-0.6B"),
            tokenizer_model_id: Some(arc("Qwen/Qwen3-Embedding-0.6B")),
            dim: 1024,
            max_len: 1024,
            query_policy: QueryPolicy::InstructionPrefix(arc(QWEN3_CODE_QUERY_PREFIX)),
            chunk_target_tokens: 768,
            chunk_hard_max_tokens: 1024,
            local_loader: Some(LocalLoaderSpec::Qwen3(Qwen3Variant::Embedding0_6B)),
        },
        EmbeddingProfile {
            name: arc("local-qwen3-4b"),
            runtime: EmbeddingRuntime::LocalQwen3CandleCuda,
            model_id: arc("Qwen/Qwen3-Embedding-4B"),
            tokenizer_model_id: Some(arc("Qwen/Qwen3-Embedding-4B")),
            dim: 2560,
            max_len: 1024,
            query_policy: QueryPolicy::InstructionPrefix(arc(QWEN3_CODE_QUERY_PREFIX)),
            chunk_target_tokens: 768,
            chunk_hard_max_tokens: 1024,
            local_loader: Some(LocalLoaderSpec::Qwen3(Qwen3Variant::Embedding4B)),
        },
        EmbeddingProfile {
            name: arc("local-qwen3-8b"),
            runtime: EmbeddingRuntime::LocalQwen3CandleCuda,
            model_id: arc("Qwen/Qwen3-Embedding-8B"),
            tokenizer_model_id: Some(arc("Qwen/Qwen3-Embedding-8B")),
            dim: 4096,
            max_len: 1024,
            query_policy: QueryPolicy::InstructionPrefix(arc(QWEN3_CODE_QUERY_PREFIX)),
            chunk_target_tokens: 768,
            chunk_hard_max_tokens: 1024,
            local_loader: Some(LocalLoaderSpec::Qwen3(Qwen3Variant::Embedding8B)),
        },
        EmbeddingProfile {
            name: arc("local-cpu-small"),
            runtime: EmbeddingRuntime::LocalFastembedOnnxCpu,
            model_id: arc("Qdrant/bge-small-en-v1.5-onnx-Q"),
            tokenizer_model_id: Some(arc("Qdrant/bge-small-en-v1.5-onnx-Q")),
            dim: 384,
            max_len: 512,
            query_policy: QueryPolicy::InstructionPrefix(arc(BGE_SEARCH_QUERY_PREFIX)),
            chunk_target_tokens: 384,
            chunk_hard_max_tokens: 512,
            local_loader: Some(LocalLoaderSpec::FastembedCpu(
                FastembedCpuModel::BgeSmallEnV15Q,
            )),
        },
        EmbeddingProfile {
            name: arc("openrouter-qwen3-8b"),
            runtime: EmbeddingRuntime::OpenRouter,
            model_id: arc("qwen/qwen3-embedding-8b"),
            tokenizer_model_id: Some(arc("Qwen/Qwen3-Embedding-8B")),
            dim: 4096,
            max_len: 32_768,
            query_policy: QueryPolicy::InstructionPrefix(arc(QWEN3_CODE_QUERY_PREFIX)),
            chunk_target_tokens: 768,
            chunk_hard_max_tokens: 1024,
            local_loader: None,
        },
    ]
});

const PROFILE_ALIASES: &[(&str, &str)] = &[
    ("qwen3-local-gpu-small", "local-gpu-small"),
    ("bge-small-cpu", "local-cpu-small"),
    ("qwen3-8b-openrouter", "openrouter-qwen3-8b"),
];

fn arc(value: &str) -> Arc<str> {
    Arc::<str>::from(value)
}

impl Qwen3Variant {
    pub fn dim(self) -> usize {
        match self {
            Self::Embedding0_6B => 1024,
            Self::Embedding4B => 2560,
            Self::Embedding8B => 4096,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Embedding0_6B => "Qwen3-Embedding-0.6B",
            Self::Embedding4B => "Qwen3-Embedding-4B",
            Self::Embedding8B => "Qwen3-Embedding-8B",
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

impl FastembedCpuModel {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::BgeSmallEnV15Q => "BGESmallENV15Q",
        }
    }

    pub fn provider_model_id(self) -> &'static str {
        match self {
            Self::BgeSmallEnV15Q => "Qdrant/bge-small-en-v1.5-onnx-Q",
        }
    }
}

impl QueryPolicy {
    pub fn format_query(&self, text: &str) -> String {
        match self {
            Self::InstructionPrefix(prefix) => format!("{}{text}", prefix.as_ref()),
            Self::None => text.to_string(),
        }
    }
}

impl EmbeddingProfile {
    pub fn name(&self) -> &str {
        self.name.as_ref()
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        let requested = s.to_ascii_lowercase();
        let canonical = PROFILE_ALIASES
            .iter()
            .find_map(|(alias, name)| (*alias == requested.as_str()).then_some(*name))
            .unwrap_or(requested.as_str());

        Self::built_in_profiles()
            .iter()
            .find(|profile| profile.name.as_ref() == canonical)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "unknown embedding profile: {s}; expected one of: {}",
                    Self::accepted_names()
                )
            })
    }

    pub fn accepted_names() -> &'static str {
        "local-gpu-small, local-cpu-small, openrouter-qwen3-8b, local-qwen3-4b, local-qwen3-8b"
    }

    pub fn default_chunk_target_tokens(&self) -> usize {
        self.chunk_target_tokens
    }

    pub fn default_chunk_hard_max_tokens(&self) -> usize {
        self.chunk_hard_max_tokens
    }

    pub(crate) fn built_in_profiles() -> &'static [Self] {
        BUILT_IN_PROFILES.as_slice()
    }
}

impl Default for EmbeddingBackend {
    fn default() -> Self {
        let profile = EmbeddingProfile::parse("local-gpu-small")
            .expect("built-in default embedding profile exists");
        Self::from_profile(profile)
    }
}

impl EmbeddingBackend {
    pub fn from_profile(profile: EmbeddingProfile) -> Self {
        Self {
            runtime: profile.runtime,
            max_len: profile.max_len,
            profile,
            force_cpu: false,
        }
    }

    pub fn from_profile_name(name: &str) -> Result<Self, EmbeddingError> {
        let profile = EmbeddingProfile::parse(name).map_err(EmbeddingError::model_init)?;
        Ok(Self::from_profile(profile))
    }

    pub fn from_qwen3_variant(variant: Qwen3Variant) -> Self {
        let profile = EmbeddingProfile::built_in_profiles()
            .iter()
            .find(|profile| {
                profile.local_loader == Some(LocalLoaderSpec::Qwen3(variant))
            })
            .cloned()
            .expect("built-in Qwen3 embedding profile exists");
        Self::from_profile(profile)
    }

    pub fn dim(&self) -> usize {
        self.profile.dim
    }

    pub fn model_id(&self) -> &str {
        self.profile.model_id.as_ref()
    }

    pub fn tokenizer_model_id(&self) -> &str {
        self.profile
            .tokenizer_model_id
            .as_deref()
            .unwrap_or_else(|| self.model_id())
    }

    pub fn model_display_name(&self) -> &str {
        match self.profile.local_loader {
            Some(LocalLoaderSpec::Qwen3(variant)) => variant.display_name(),
            Some(LocalLoaderSpec::FastembedCpu(model)) => model.display_name(),
            None => self.model_id(),
        }
    }

    pub fn qwen3_variant(&self) -> Option<Qwen3Variant> {
        match self.profile.local_loader {
            Some(LocalLoaderSpec::Qwen3(variant)) => Some(variant),
            _ => None,
        }
    }

    pub fn require_qwen3_variant(&self) -> Result<Qwen3Variant, EmbeddingError> {
        self.qwen3_variant().ok_or_else(|| {
            EmbeddingError::model_init(format!(
                "embedding profile `{}` does not use the local Qwen3 runtime",
                self.profile.name()
            ))
        })
    }

    pub fn fastembed_cpu_model(&self) -> Option<FastembedCpuModel> {
        match self.profile.local_loader {
            Some(LocalLoaderSpec::FastembedCpu(model)) => Some(model),
            _ => None,
        }
    }

    pub fn require_fastembed_cpu_model(&self) -> Result<FastembedCpuModel, EmbeddingError> {
        self.fastembed_cpu_model().ok_or_else(|| {
            EmbeddingError::model_init(format!(
                "embedding profile `{}` does not use the fastembed ONNX CPU runtime",
                self.profile.name()
            ))
        })
    }

    pub fn format_query(&self, text: &str) -> String {
        self.profile.query_policy.format_query(text)
    }

    /// Stable string used in cache paths and EMBEDDER_VERSION.
    ///
    /// The default local Qwen3-Embedding-0.6B identity intentionally keeps the
    /// existing `v2` string so current indexes remain compatible.
    pub fn identity(&self) -> String {
        match self.runtime {
            EmbeddingRuntime::LocalQwen3CandleCuda => format!(
                "fastembed-candle:{}:dim{}:max{}:v2",
                self.model_display_name(),
                self.dim(),
                self.max_len,
            ),
            EmbeddingRuntime::LocalFastembedOnnxCpu => format!(
                "fastembed-onnx-cpu:{}:dim{}:max{}:v1",
                self.model_display_name(),
                self.dim(),
                self.max_len,
            ),
            EmbeddingRuntime::OpenRouter => format!(
                "openrouter:{}:dim{}:max{}:v1",
                self.model_id(),
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
                    "BGESmallENV15Q" => Self::from_profile_name("local-cpu-small")?,
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
                        Self::from_profile_name("openrouter-qwen3-8b")?
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
                "dim `{}` does not match profile `{}` (expected {}) in `{}`",
                dim,
                backend.profile.name(),
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

    fn profile(name: &str) -> EmbeddingProfile {
        EmbeddingProfile::parse(name).unwrap()
    }

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
        assert_eq!(EmbeddingBackend::from_profile(profile("local-cpu-small")).dim(), 384);
        assert_eq!(
            EmbeddingBackend::from_profile(profile("openrouter-qwen3-8b")).dim(),
            4096
        );
        assert_eq!(Qwen3Variant::Embedding4B.dim(), 2560);
        assert_eq!(Qwen3Variant::Embedding8B.dim(), 4096);
    }

    #[test]
    fn built_in_profile_data_matches_previous_values() {
        let local_gpu = profile("local-gpu-small");
        assert_eq!(local_gpu.model_id.as_ref(), "Qwen/Qwen3-Embedding-0.6B");
        assert_eq!(local_gpu.max_len, 1024);
        assert_eq!(local_gpu.chunk_target_tokens, 768);
        assert_eq!(local_gpu.chunk_hard_max_tokens, 1024);

        let local_cpu = profile("local-cpu-small");
        assert_eq!(
            local_cpu.model_id.as_ref(),
            "Qdrant/bge-small-en-v1.5-onnx-Q"
        );
        assert_eq!(local_cpu.dim, 384);
        assert_eq!(local_cpu.max_len, 512);

        let openrouter = profile("openrouter-qwen3-8b");
        assert_eq!(openrouter.model_id.as_ref(), "qwen/qwen3-embedding-8b");
        assert_eq!(
            openrouter.tokenizer_model_id.as_deref(),
            Some("Qwen/Qwen3-Embedding-8B")
        );
    }

    #[test]
    fn identities_are_unique_by_profile() {
        let mut identities = std::collections::HashSet::new();

        for profile in EmbeddingProfile::built_in_profiles().iter().cloned() {
            assert!(identities.insert(EmbeddingBackend::from_profile(profile).identity()));
        }
    }

    #[test]
    fn profile_parse_accepts_explicit_profiles() {
        assert_eq!(
            EmbeddingProfile::parse("local-gpu-small").unwrap().name(),
            "local-gpu-small"
        );
        assert_eq!(
            EmbeddingProfile::parse("local-cpu-small").unwrap().name(),
            "local-cpu-small"
        );
        assert_eq!(
            EmbeddingProfile::parse("openrouter-qwen3-8b").unwrap().name(),
            "openrouter-qwen3-8b"
        );
    }

    #[test]
    fn profile_parse_accepts_legacy_aliases() {
        assert_eq!(
            EmbeddingProfile::parse("qwen3-local-gpu-small")
                .unwrap()
                .name(),
            "local-gpu-small"
        );
        assert_eq!(
            EmbeddingProfile::parse("bge-small-cpu").unwrap().name(),
            "local-cpu-small"
        );
        assert_eq!(
            EmbeddingProfile::parse("qwen3-8b-openrouter")
                .unwrap()
                .name(),
            "openrouter-qwen3-8b"
        );
    }

    #[test]
    fn query_policy_is_profile_aware() {
        assert_eq!(
            EmbeddingBackend::from_profile(profile("local-gpu-small"))
                .format_query("find parser"),
            "Instruct: Given a code search query, retrieve relevant code\nQuery: find parser"
        );
        assert_eq!(
            EmbeddingBackend::from_profile(profile("local-cpu-small"))
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
        let original = EmbeddingBackend::from_profile(profile("local-cpu-small"));
        let parsed = EmbeddingBackend::from_identity(&original.identity()).unwrap();
        assert_eq!(parsed.profile, original.profile);
        assert_eq!(parsed.max_len, original.max_len);
    }

    #[test]
    fn from_identity_roundtrip_openrouter_profile() {
        let original = EmbeddingBackend::from_profile(profile("openrouter-qwen3-8b"));
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
