//! Embedding backend runtime wiring.
//!
//! [`EmbeddingBackend`] wraps an [`EmbeddingProfile`] with per-instance
//! runtime state (`max_len` override, `force_cpu`) and exposes the
//! stable identity string used in cache paths and `EMBEDDER_VERSION`.
//!
//! The profile data model itself — `EmbeddingProfile`, `QueryPolicy`,
//! `LocalLoaderSpec`, `FastembedOnnxModel`, `Qwen3Variant`, and the
//! built-in profile registry — lives in [`super::profile`].

use super::batching::{BatchRows, FixedInputShape};
use super::error::EmbeddingError;
use super::identity::EmbeddingIdentity;
use super::profile::{
    EmbeddingProfile, FastembedOnnxModel, LocalLoaderSpec, QueryPolicy, Qwen3Variant,
};
use super::util::arc;

/// Batch height that the MIGraphX kernels are compiled for.
///
/// # Why the shape is constant
/// MIGraphX compiles kernels FOR THE INPUT SHAPE: every new pair
/// (rows × length) costs 45–70 s of compilation and ~145–200 MB in the `.mxr` cache.
/// The shape fastembed produces by default floats along both axes
/// (padding to the longest row IN THE BATCH + an incomplete last batch), and a single
/// indexing run over 40 files produced 4 shapes and 659 MB of cache. Fixing it leaves one.
///
/// # Why exactly 32
/// It is the height at which the GPU path's ceiling was measured (242 seq/s vs 7.5 on CPU).
/// The number is deliberately NOT derived from the input: the point is that the shape does not depend
/// on how many texts arrived.
///
/// The coincidence with the indexer's default `gpu_batch_size` NO LONGER carries any weight:
/// with a constant shape the indexer takes the height from here and does not apply its own setting
/// at all.
const GPU_BATCH_ROWS: BatchRows = BatchRows(32);

/// Cross-crate embedding runtime boundary.
///
/// Indexing, graph, and server crates use this type when they need a concrete
/// embedding runtime choice, cache identity, or dimension contract. Backend
/// construction and identity parsing remain engine-owned so higher crates do
/// not duplicate runtime-specific policy.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EmbeddingBackend {
    pub profile: EmbeddingProfile,
    pub runtime: EmbeddingRuntime,
    pub max_len: usize,
    /// Off by default. Set only for CI/benchmark runs. Enabling this
    /// emits a warn! on every construction.
    pub force_cpu: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EmbeddingRuntime {
    LocalQwen3CandleCuda,
    LocalFastembedOnnxCpu,
    /// The same ONNX graph as `LocalFastembedOnnxCpu`, but executed on an AMD
    /// GPU via the MIGraphX EP.
    ///
    /// A separate variant rather than a flag on the CPU runtime BECAUSE it goes into
    /// `EmbeddingIdentity` and therefore separates indexes. The separation here is not
    /// overcaution: the GPU profile runs an fp32 model, while the CPU profile runs
    /// an int8-quantized one, i.e. the vectors are DIFFERENT, and mixing them in one
    /// index is not allowed.
    LocalFastembedOnnxMigraphx,
    /// The same ONNX graph, executed on GPU via the **DirectML EP** — the WINDOWS path.
    ///
    /// A separate variant for the same reason as MIGraphX (it is in
    /// `EmbeddingIdentity` ⇒ separates indexes), and additionally because these are
    /// DIFFERENT runtimes with different arithmetic: DirectML computes fp16 where
    /// MIGraphX computes fp32, so vectors from the two GPU paths must not be mixed
    /// any more than GPU and CPU vectors.
    ///
    /// Why not MIGraphX on Windows: it does not exist there at all — not the whole ROCm
    /// stack has been ported to Windows, and MIGraphX is not part of what was ported.
    LocalFastembedOnnxDirectml,
    OpenRouter,
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
            .find(|profile| profile.local_loader == Some(LocalLoaderSpec::Qwen3(variant)))
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
            Some(LocalLoaderSpec::FastembedOnnx(model)) => model.display_name(),
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

    pub fn fastembed_onnx_model(&self) -> Option<FastembedOnnxModel> {
        match self.profile.local_loader {
            Some(LocalLoaderSpec::FastembedOnnx(model)) => Some(model),
            _ => None,
        }
    }

    /// Whether this backend goes through fastembed/ONNX — on CPU or on GPU.
    pub fn is_fastembed_onnx(&self) -> bool {
        matches!(
            self.runtime,
            EmbeddingRuntime::LocalFastembedOnnxCpu
                | EmbeddingRuntime::LocalFastembedOnnxMigraphx
                | EmbeddingRuntime::LocalFastembedOnnxDirectml
        )
    }

    pub fn require_fastembed_onnx_model(&self) -> Result<FastembedOnnxModel, EmbeddingError> {
        self.fastembed_onnx_model().ok_or_else(|| {
            EmbeddingError::model_init(format!(
                "embedding profile `{}` does not use the fastembed ONNX CPU runtime",
                self.profile.name()
            ))
        })
    }

    /// Constant input shape, if this profile's runtime REQUIRES one.
    ///
    /// # Single source of the shape
    /// The shape is read by two parties: the model loader (compiles kernels for it and
    /// addresses the cache directory by it) and the indexer (cuts inputs by it). While each
    /// knew its own constant, their consistency rested on the coincidence of
    /// defaults — see [`super::batching::BatchingPolicy`]. Now the value
    /// is declared by ONE side, and the other accepts it.
    ///
    /// `None` does not mean 'shape unknown' but 'the runtime does not need a shape': on CPU and for
    /// API models padding to the longest row in the batch is free.
    pub fn fixed_input_shape(&self) -> Option<FixedInputShape> {
        match self.runtime {
            // MIGraphX compiles kernels for the shape; DirectML does not compile kernels,
            // but likewise rebuilds the graph for every new shape, and its
            // own documentation says outright that the EP works best
            // when input sizes are known at session creation. For both we
            // DECLARE the shape — the only difference is the cost of violating it.
            EmbeddingRuntime::LocalFastembedOnnxMigraphx
            | EmbeddingRuntime::LocalFastembedOnnxDirectml => Some(FixedInputShape {
                rows: GPU_BATCH_ROWS,
                seq_len: self.max_len,
            }),
            EmbeddingRuntime::LocalQwen3CandleCuda
            | EmbeddingRuntime::LocalFastembedOnnxCpu
            | EmbeddingRuntime::OpenRouter => None,
        }
    }

    /// Whether this backend goes through fastembed/ONNX ON GPU.
    ///
    /// Collected in one place on purpose: there are now two GPU runtimes (MIGraphX on
    /// Linux, DirectML on Windows), and any `== LocalFastembedOnnxMigraphx` in
    /// the sense of 'this is GPU' became a BUG once the second one appeared — it would silently answer
    /// 'no' on the Windows path.
    pub fn is_fastembed_onnx_gpu(&self) -> bool {
        matches!(
            self.runtime,
            EmbeddingRuntime::LocalFastembedOnnxMigraphx
                | EmbeddingRuntime::LocalFastembedOnnxDirectml
        )
    }

    /// Whether a run of this backend executes on the GPU: the fastembed GPU
    /// runtimes plus Qwen3 on CUDA. Decides who takes the cross-process GPU
    /// lock (`gpu_lock`).
    pub fn runs_on_gpu(&self) -> bool {
        self.is_fastembed_onnx_gpu() || self.runtime == EmbeddingRuntime::LocalQwen3CandleCuda
    }

    pub fn format_query(&self, text: &str) -> String {
        self.profile.query_policy.format_query(text)
    }

    /// Default cosine-similarity cutoff for the `semantic_overlaps`
    /// duplicate-detection audit.
    ///
    /// Cosine-similarity score distributions are model-specific, so a fixed
    /// cutoff is only meaningful relative to the model that produced the
    /// vectors. This derives the default from the active model instead of
    /// leaving a bare literal at the call site; callers may always pass an
    /// explicit threshold to override it.
    pub fn semantic_overlap_threshold(&self) -> f32 {
        match self.profile.local_loader {
            // Qwen3 code-embedding family: instruction-tuned, related code
            // clusters tightly at high cosine similarity.
            Some(LocalLoaderSpec::Qwen3(_)) => 0.85,
            // BGE general-purpose sentence embeddings sit on a lower
            // similarity scale than instruction-tuned code embeddings.
            Some(LocalLoaderSpec::FastembedOnnx(_)) => 0.80,
            // API models have no local loader. The built-in OpenRouter
            // Qwen3 model shares the Qwen3 scale; other API models (e.g.
            // OpenAI text-embedding-3, whose similarity range is markedly
            // compressed) should be given an explicit threshold until a
            // value is measured for them.
            None => 0.85,
        }
    }

    /// Stable string used in cache paths and EMBEDDER_VERSION.
    pub fn identity(&self) -> String {
        EmbeddingIdentity {
            runtime: self.runtime,
            model_id: self.model_id().to_string(),
            dim: self.dim(),
            max_len: self.max_len,
            query: self.profile.query_policy.encode_tag(),
        }
        .encode()
    }

    /// Parse an `identity()` string back into an `EmbeddingBackend`.
    ///
    /// Used to reconcile the embedder recorded in a vector store's
    /// `metadata.json` with the embedder a search-time caller wants.
    pub fn from_identity(s: &str) -> Result<Self, EmbeddingError> {
        if s.starts_with("emb;") {
            return Self::from_v2_identity(s);
        }

        Self::from_legacy_identity(s)
    }

    fn from_v2_identity(s: &str) -> Result<Self, EmbeddingError> {
        let identity = EmbeddingIdentity::decode(s).map_err(EmbeddingError::invalid_identity)?;
        let query_policy =
            QueryPolicy::decode_tag(&identity.query).map_err(EmbeddingError::invalid_identity)?;

        match identity.runtime {
            EmbeddingRuntime::OpenRouter => {
                let mut profile = EmbeddingProfile::built_in_api_for_identity(
                    EmbeddingRuntime::OpenRouter,
                    &identity.model_id,
                )
                .unwrap_or_else(|| EmbeddingProfile {
                    name: arc(&format!("openrouter:{}", identity.model_id)),
                    runtime: EmbeddingRuntime::OpenRouter,
                    model_id: arc(&identity.model_id),
                    tokenizer_model_id: None,
                    dim: identity.dim,
                    max_len: identity.max_len,
                    query_policy: query_policy.clone(),
                    chunk_target_tokens: 768,
                    chunk_hard_max_tokens: 1024,
                    local_loader: None,
                });
                profile.dim = identity.dim;
                profile.max_len = identity.max_len;
                profile.query_policy = query_policy;
                profile.local_loader = None;
                Ok(Self::from_profile(profile))
            }
            runtime => {
                let mut profile = EmbeddingProfile::built_in_local_for_identity(
                    runtime,
                    &identity.model_id,
                )
                .ok_or_else(|| {
                    EmbeddingError::invalid_identity(format!(
                        "local embedding identity references unknown built-in model `{}` for runtime {:?}; \
                         run `clear_cache` for this directory to discard the stale or foreign index",
                        identity.model_id, runtime
                    ))
                })?;
                if identity.dim != profile.dim {
                    return Err(EmbeddingError::invalid_identity(format!(
                        "dim `{}` does not match built-in profile `{}` (expected {}) in `{}`",
                        identity.dim,
                        profile.name(),
                        profile.dim,
                        s
                    )));
                }
                profile.query_policy = query_policy;
                let mut backend = Self::from_profile(profile);
                backend.max_len = identity.max_len;
                Ok(backend)
            }
        }
    }

    fn from_legacy_identity(s: &str) -> Result<Self, EmbeddingError> {
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
                    "qwen/qwen3-embedding-8b" => Self::from_profile_name("openrouter-qwen3-8b")?,
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
    use super::super::profile::QWEN3_CODE_QUERY_PREFIX;
    use super::*;

    fn profile(name: &str) -> EmbeddingProfile {
        EmbeddingProfile::parse(name).unwrap()
    }

    #[test]
    fn default_backend_dim_is_1024() {
        assert_eq!(EmbeddingBackend::default().dim(), 1024);
    }

    #[test]
    fn default_backend_identity_uses_v2_codec() {
        let identity = EmbeddingBackend::default().identity();
        let decoded = EmbeddingIdentity::decode(&identity).unwrap();

        assert!(identity.starts_with("emb;v=2;"));
        assert_eq!(decoded.runtime, EmbeddingRuntime::LocalQwen3CandleCuda);
        assert_eq!(decoded.model_id, "Qwen/Qwen3-Embedding-0.6B");
        assert_eq!(decoded.dim, 1024);
        assert_eq!(decoded.max_len, 1024);
    }

    #[test]
    fn profile_dimensions_match_expected_values() {
        assert_eq!(
            EmbeddingBackend::from_profile(profile("local-cpu-small")).dim(),
            384
        );
        assert_eq!(
            EmbeddingBackend::from_profile(profile("openrouter-qwen3-8b")).dim(),
            4096
        );
        assert_eq!(Qwen3Variant::Embedding4B.dim(), 2560);
        assert_eq!(Qwen3Variant::Embedding8B.dim(), 4096);
    }

    /// Exactly the profiles whose runs execute on a GPU take the cross-process
    /// GPU lock; a CPU or remote profile waiting behind another daemon's GPU
    /// batch would be a slowdown with no crash to prevent.
    #[test]
    fn only_gpu_profiles_take_the_gpu_lock() {
        for profile in EmbeddingProfile::built_in_profiles().iter().cloned() {
            let backend = EmbeddingBackend::from_profile(profile);
            let gpu = matches!(
                backend.runtime,
                EmbeddingRuntime::LocalQwen3CandleCuda
                    | EmbeddingRuntime::LocalFastembedOnnxMigraphx
                    | EmbeddingRuntime::LocalFastembedOnnxDirectml
            );
            assert_eq!(backend.runs_on_gpu(), gpu, "{:?}", backend.runtime);
        }
    }

    #[test]
    fn identities_are_unique_by_profile() {
        let mut identities = std::collections::HashSet::new();

        for profile in EmbeddingProfile::built_in_profiles().iter().cloned() {
            assert!(identities.insert(EmbeddingBackend::from_profile(profile).identity()));
        }
    }

    #[test]
    fn query_policy_is_profile_aware() {
        assert_eq!(
            EmbeddingBackend::from_profile(profile("local-gpu-small")).format_query("find parser"),
            "Instruct: Given a code search query, retrieve relevant code\nQuery: find parser"
        );
        assert_eq!(
            EmbeddingBackend::from_profile(profile("local-cpu-small")).format_query("find parser"),
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
        assert_eq!(parsed.runtime, original.runtime);
        assert_eq!(parsed.model_id(), original.model_id());
        assert_eq!(parsed.dim(), original.dim());
        assert_eq!(parsed.max_len, original.max_len);
        assert_eq!(parsed.profile.query_policy, original.profile.query_policy);
    }

    #[test]
    fn from_identity_roundtrips_all_built_in_profiles() {
        for profile in EmbeddingProfile::built_in_profiles().iter().cloned() {
            let original = EmbeddingBackend::from_profile(profile);
            let parsed = EmbeddingBackend::from_identity(&original.identity()).unwrap();

            assert_eq!(parsed.runtime, original.runtime);
            assert_eq!(parsed.model_id(), original.model_id());
            assert_eq!(parsed.dim(), original.dim());
            assert_eq!(parsed.max_len, original.max_len);
            assert_eq!(parsed.profile.query_policy, original.profile.query_policy);
        }
    }

    #[test]
    fn from_identity_roundtrips_api_model_ids_with_reserved_chars() {
        let profile = EmbeddingProfile {
            name: arc("dynamic-openrouter"),
            runtime: EmbeddingRuntime::OpenRouter,
            model_id: arc("provider/model:revision"),
            tokenizer_model_id: None,
            dim: 1536,
            max_len: 8192,
            query_policy: QueryPolicy::InputType {
                document: arc("document=type;v1"),
                query: arc("query/type\nv1"),
            },
            chunk_target_tokens: 768,
            chunk_hard_max_tokens: 1024,
            local_loader: None,
        };
        let original = EmbeddingBackend::from_profile(profile);
        let parsed = EmbeddingBackend::from_identity(&original.identity()).unwrap();

        assert_eq!(parsed.runtime, EmbeddingRuntime::OpenRouter);
        assert_eq!(parsed.model_id(), "provider/model:revision");
        assert_eq!(parsed.dim(), 1536);
        assert_eq!(parsed.max_len, 8192);
        assert_eq!(parsed.profile.query_policy, original.profile.query_policy);
    }

    #[test]
    fn from_identity_accepts_legacy_identities() {
        let default = "fastembed-candle:Qwen3-Embedding-0.6B:dim1024:max1024:v2";
        let cpu = "fastembed-onnx-cpu:BGESmallENV15Q:dim384:max512:v1";
        let openrouter = "openrouter:qwen/qwen3-embedding-8b:dim4096:max32768:v1";

        assert_eq!(
            EmbeddingBackend::from_identity(default)
                .unwrap()
                .profile
                .name(),
            "local-gpu-small"
        );
        assert_eq!(
            EmbeddingBackend::from_identity(cpu).unwrap().profile.name(),
            "local-cpu-small"
        );
        assert_eq!(
            EmbeddingBackend::from_identity(openrouter)
                .unwrap()
                .profile
                .name(),
            "openrouter-qwen3-8b"
        );
    }

    #[test]
    fn from_identity_rejects_unknown_v2_local_model() {
        let identity = EmbeddingIdentity {
            runtime: EmbeddingRuntime::LocalQwen3CandleCuda,
            model_id: "Qwen/Unknown".to_string(),
            dim: 1024,
            max_len: 1024,
            query: QueryPolicy::InstructionPrefix(arc(QWEN3_CODE_QUERY_PREFIX)).encode_tag(),
        }
        .encode();
        let err = EmbeddingBackend::from_identity(&identity).unwrap_err();
        let text = err.to_string();

        assert!(text.contains("unknown built-in model"));
        assert!(text.contains("clear_cache"));
    }

    #[test]
    fn from_identity_rejects_garbage() {
        assert!(EmbeddingBackend::from_identity("garbage").is_err());
        assert!(
            EmbeddingBackend::from_identity(
                "fastembed-candle:Qwen3-Embedding-0.6B:dim999:max2048:v2"
            )
            .is_err()
        );
        assert!(
            EmbeddingBackend::from_identity(
                "fastembed-candle:Qwen3-Embedding-0.6B:dim1024:max1024:v1"
            )
            .is_err()
        );
    }

    /// Gate for the pair 'runtime ⇄ constant shape': a shape exists for EXACTLY those
    /// runtimes that execute the graph on GPU.
    ///
    /// Introduced because the halves of the pair live in different files: the model loader
    /// refuses if the runtime is a GPU one and there is no shape — but the reverse
    /// skew (a shape declared for a runtime that does not need it) would go unnoticed by
    /// anyone except this test. It also keeps coverage: a new runtime must
    /// appear in the `match` explicitly, otherwise `fixed_input_shape` will not compile.
    #[test]
    fn fixed_shape_exists_exactly_for_shape_compiling_runtimes() {
        let profiles = EmbeddingProfile::built_in_profiles();
        let mut seen_gpu = false;
        for profile in profiles.iter() {
            let backend = EmbeddingBackend::from_profile(profile.clone());
            let shape = backend.fixed_input_shape();
            let wants_shape = backend.is_fastembed_onnx_gpu();
            assert_eq!(
                shape.is_some(),
                wants_shape,
                "profile `{}` ({:?}): shape and runtime diverged",
                profile.name(),
                backend.runtime
            );
            if let Some(shape) = shape {
                seen_gpu = true;
                assert_eq!(shape.rows, GPU_BATCH_ROWS);
                // The sequence length is the backend's `max_len`, not a
                // constant: it is overridden per instance, and a mismatch with it
                // would mean compiling kernels for a shape that never occurs.
                assert_eq!(shape.seq_len, backend.max_len);
            }
        }
        assert!(
            seen_gpu,
            "no GPU profile left in the registry — the test has become vacuous"
        );
    }
}
