//! Operational defaults for MCP server startup and automatic work.

use rmc_engine::embeddings::{EmbeddingBackend, EmbeddingRuntime};

pub const BACKGROUND_SYNC_ENV: &str = "RMC_BACKGROUND_SYNC";
pub const BACKGROUND_SYNC_ENABLED_VALUES: &str = "1/true/yes/on";
pub const AUTOMATIC_EMBEDDING_PROFILE: &str = "local-cpu-small";

pub fn parse_background_sync_env(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return false;
    };

    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

pub fn automatic_embedding_profile_name() -> &'static str {
    AUTOMATIC_EMBEDDING_PROFILE
}

pub(crate) fn automatic_embedding_backend() -> EmbeddingBackend {
    EmbeddingBackend::from_profile_name(AUTOMATIC_EMBEDDING_PROFILE)
        .expect("built-in automatic embedding profile exists")
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
    fn automatic_embedding_backend_is_cpu_profile() {
        let backend = automatic_embedding_backend();

        assert_eq!(backend.profile.name(), "local-cpu-small");
        assert!(is_background_embedding_backend(&backend));
    }
}
