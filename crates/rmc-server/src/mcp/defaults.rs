//! Operational defaults for MCP server startup and automatic work.

use rmc_engine::embeddings::{EmbeddingBackend, EmbeddingProfile, EmbeddingRuntime};
use std::sync::OnceLock;

pub const BACKGROUND_SYNC_ENV: &str = "RMC_BACKGROUND_SYNC";
pub const BACKGROUND_SYNC_ENABLED_VALUES: &str = "1/true/yes/on";

/// Names the default built-in profile. Not the same knob as
/// `RUST_CODE_MCP_EMBEDDING_PROFILES`, which points at a TOML file of extra
/// profile definitions.
pub const EMBEDDING_PROFILE_ENV: &str = "RMC_EMBEDDING_PROFILE";

/// Used when [`EMBEDDING_PROFILE_ENV`] is unset; compiled into every build.
pub const DEFAULT_AUTOMATIC_EMBEDDING_PROFILE: &str = "local-cpu-small";

static AUTOMATIC_BACKEND: OnceLock<EmbeddingBackend> = OnceLock::new();

pub fn parse_background_sync_env(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return false;
    };

    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// A blank value counts as unset: an empty value from a launch wrapper is a
/// slip, not a profile name.
pub fn resolve_automatic_profile_name<'a>(env_value: Option<&'a str>) -> &'a str {
    env_value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_AUTOMATIC_EMBEDDING_PROFILE)
}

/// Checks the name against the built-in profiles, then against the compiled
/// features: a CUDA profile in a CPU-only build must not pass startup and fail
/// later at model init, with indexing speed as the only symptom.
pub fn validate_automatic_profile(name: &str) -> Result<EmbeddingBackend, String> {
    let backend = EmbeddingBackend::from_profile_name(name).map_err(|_| {
        format!(
            "unusable value `{name}`; only built-in profile names are accepted here, one of: {}. \
             A profile defined in embedding_profiles.toml stays reachable through the \
             `embedding_profile` tool argument, not through this variable",
            EmbeddingProfile::accepted_names()
        )
    })?;

    if backend.runtime == EmbeddingRuntime::LocalQwen3CandleCuda
        && !rmc_engine::embeddings::CUDA_CAPABLE_FEATURES_COMPILED
    {
        return Err(format!(
            "profile `{}` needs the local CUDA runtime, but this binary was built without the \
             `cuda` feature",
            backend.profile.name()
        ));
    }

    Ok(backend)
}

/// Installs the validated backend as the process-wide default. The error means
/// something read the default before startup finished, so the returned backend
/// was not installed and does not decide what the run uses.
pub fn install_automatic_backend(backend: EmbeddingBackend) -> Result<(), EmbeddingBackend> {
    AUTOMATIC_BACKEND.set(backend)
}

/// The closure runs only when startup installed nothing, for library callers
/// and tests, and still refuses a bad value.
fn automatic_backend_ref() -> &'static EmbeddingBackend {
    AUTOMATIC_BACKEND.get_or_init(|| {
        let requested = std::env::var(EMBEDDING_PROFILE_ENV).ok();
        let name = resolve_automatic_profile_name(requested.as_deref());
        match validate_automatic_profile(name) {
            Ok(backend) => backend,
            Err(err) => panic!("{EMBEDDING_PROFILE_ENV}: {err}"),
        }
    })
}

/// Cheap clone: the fields are `Arc`s, so no profile name is parsed per call.
pub fn automatic_embedding_backend() -> EmbeddingBackend {
    automatic_backend_ref().clone()
}

pub fn automatic_embedding_profile_name() -> &'static str {
    automatic_backend_ref().profile.name()
}

pub fn cuda_capable_features_compiled() -> bool {
    rmc_engine::embeddings::CUDA_CAPABLE_FEATURES_COMPILED
}

pub(crate) fn is_background_embedding_backend(backend: &EmbeddingBackend) -> bool {
    matches!(
        backend.runtime,
        EmbeddingRuntime::LocalFastembedOnnxCpu | EmbeddingRuntime::OpenRouter
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_sync_env_is_disabled_by_default() {
        assert!(!parse_background_sync_env(None));
        assert!(!parse_background_sync_env(Some("")));
        assert!(!parse_background_sync_env(Some("0")));
        assert!(!parse_background_sync_env(Some("false")));
    }

    #[test]
    fn background_sync_env_accepts_explicit_true_values() {
        assert!(parse_background_sync_env(Some("1")));
        assert!(parse_background_sync_env(Some("true")));
        assert!(parse_background_sync_env(Some("YES")));
        assert!(parse_background_sync_env(Some(" on ")));
    }

    #[test]
    fn profile_env_is_trimmed_and_blank_counts_as_unset() {
        for blank in [None, Some(""), Some("   ")] {
            assert_eq!(
                resolve_automatic_profile_name(blank),
                DEFAULT_AUTOMATIC_EMBEDDING_PROFILE
            );
        }

        assert_eq!(
            resolve_automatic_profile_name(Some(" local-gpu-small\n")),
            "local-gpu-small"
        );
    }

    #[test]
    fn default_profile_is_a_usable_cpu_backend() {
        let backend = validate_automatic_profile(DEFAULT_AUTOMATIC_EMBEDDING_PROFILE)
            .expect("built-in CPU profile is always usable");

        assert_eq!(backend.profile.name(), "local-cpu-small");
        assert!(is_background_embedding_backend(&backend));
    }

    #[test]
    fn unknown_profile_is_refused_and_names_the_value() {
        let err = validate_automatic_profile("local-gpu-small-typo")
            .expect_err("a misspelled profile name is not built in");

        assert!(err.contains("local-gpu-small-typo"), "message was: {err}");
    }

    #[cfg(not(feature = "cuda"))]
    #[test]
    fn cuda_profile_is_refused_without_the_cuda_feature() {
        let err = validate_automatic_profile("local-gpu-small")
            .expect_err("a CUDA profile is unusable in a CPU-only build");

        assert!(err.contains("cuda"), "message was: {err}");
    }

    #[cfg(feature = "cuda")]
    #[test]
    fn cuda_profile_is_accepted_with_the_cuda_feature() {
        let backend = validate_automatic_profile("local-gpu-small")
            .expect("a CUDA profile is usable in a CUDA build");

        assert_eq!(backend.runtime, EmbeddingRuntime::LocalQwen3CandleCuda);
    }
}
