//! Embedding profile data model and the built-in profile registry.
//!
//! Defines [`EmbeddingProfile`] (the static description of an embedding
//! model: runtime, model id, dim, tokenization, query policy) and the
//! enums that name the local loaders and Qwen3 variants. The runtime
//! [`super::backend::EmbeddingBackend`] wraps a profile and adds
//! per-instance state like `force_cpu` and `max_len` overrides.
//!
//! The built-in profile registry lives here as a `LazyLock<Vec<...>>`
//! plus an alias table for legacy profile names.

use super::backend::EmbeddingRuntime;
use super::identity::{percent_decode, percent_encode};
use super::util::arc;
use std::sync::{Arc, LazyLock};

pub(crate) const QWEN3_CODE_QUERY_PREFIX: &str =
    "Instruct: Given a code search query, retrieve relevant code\nQuery: ";
pub(crate) const BGE_SEARCH_QUERY_PREFIX: &str =
    "Represent this sentence for searching relevant passages: ";

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
    InputType { document: Arc<str>, query: Arc<str> },
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
            query_policy: QueryPolicy::InputType {
                document: arc("search_document"),
                query: arc("search_query"),
            },
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
            Self::InputType { .. } | Self::None => text.to_string(),
        }
    }

    pub fn input_types(&self) -> Option<(&str, &str)> {
        match self {
            Self::InputType { document, query } => Some((document.as_ref(), query.as_ref())),
            _ => None,
        }
    }

    pub fn encode_tag(&self) -> String {
        match self {
            Self::InstructionPrefix(prefix) => {
                format!("prefix:{}", percent_encode(prefix.as_ref()))
            }
            Self::InputType { document, query } => format!(
                "input-type:{}:{}",
                percent_encode(document.as_ref()),
                percent_encode(query.as_ref())
            ),
            Self::None => "none".to_string(),
        }
    }

    pub fn decode_tag(tag: &str) -> Result<Self, String> {
        if tag == "none" {
            return Ok(Self::None);
        }
        if let Some(encoded) = tag.strip_prefix("prefix:") {
            return Ok(Self::InstructionPrefix(arc(&percent_decode(encoded)?)));
        }
        if let Some(encoded) = tag.strip_prefix("input-type:") {
            let (document, query) = encoded.split_once(':').ok_or_else(|| {
                format!("malformed query policy input-type tag `{tag}`")
            })?;
            return Ok(Self::InputType {
                document: arc(&percent_decode(document)?),
                query: arc(&percent_decode(query)?),
            });
        }

        Err(format!("unknown query policy tag `{tag}`"))
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

    pub(super) fn built_in_local_for_identity(
        runtime: EmbeddingRuntime,
        model_id: &str,
    ) -> Option<Self> {
        Self::built_in_profiles()
            .iter()
            .find(|profile| {
                profile.runtime == runtime
                    && profile.model_id.as_ref() == model_id
                    && profile.local_loader.is_some()
            })
            .cloned()
    }

    pub(super) fn built_in_api_for_identity(
        runtime: EmbeddingRuntime,
        model_id: &str,
    ) -> Option<Self> {
        Self::built_in_profiles()
            .iter()
            .find(|profile| {
                profile.runtime == runtime
                    && profile.model_id.as_ref() == model_id
                    && profile.local_loader.is_none()
            })
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_profile_data_matches_previous_values() {
        let local_gpu = EmbeddingProfile::parse("local-gpu-small").unwrap();
        assert_eq!(local_gpu.model_id.as_ref(), "Qwen/Qwen3-Embedding-0.6B");
        assert_eq!(local_gpu.max_len, 1024);
        assert_eq!(local_gpu.chunk_target_tokens, 768);
        assert_eq!(local_gpu.chunk_hard_max_tokens, 1024);

        let local_cpu = EmbeddingProfile::parse("local-cpu-small").unwrap();
        assert_eq!(
            local_cpu.model_id.as_ref(),
            "Qdrant/bge-small-en-v1.5-onnx-Q"
        );
        assert_eq!(local_cpu.dim, 384);
        assert_eq!(local_cpu.max_len, 512);

        let openrouter = EmbeddingProfile::parse("openrouter-qwen3-8b").unwrap();
        assert_eq!(openrouter.model_id.as_ref(), "qwen/qwen3-embedding-8b");
        assert_eq!(
            openrouter.tokenizer_model_id.as_deref(),
            Some("Qwen/Qwen3-Embedding-8B")
        );
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
    fn query_policy_tags_roundtrip() {
        let policies = [
            QueryPolicy::InstructionPrefix(arc("prefix /:=;\n")),
            QueryPolicy::InputType {
                document: arc("doc=type;v1"),
                query: arc("query/type\nv1"),
            },
            QueryPolicy::None,
        ];

        for policy in policies {
            let tag = policy.encode_tag();
            let decoded = QueryPolicy::decode_tag(&tag).unwrap();
            assert_eq!(decoded, policy);
        }
    }

    #[test]
    fn openrouter_profile_uses_input_type_policy() {
        let profile = EmbeddingProfile::parse("openrouter-qwen3-8b").unwrap();

        assert_eq!(
            profile.query_policy.input_types(),
            Some(("search_document", "search_query"))
        );
        assert_eq!(
            profile.query_policy.format_query("find parser"),
            "find parser"
        );
    }

    #[test]
    fn qwen3_variant_dims() {
        assert_eq!(Qwen3Variant::Embedding0_6B.dim(), 1024);
        assert_eq!(Qwen3Variant::Embedding4B.dim(), 2560);
        assert_eq!(Qwen3Variant::Embedding8B.dim(), 4096);
    }
}
