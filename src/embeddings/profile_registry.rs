//! Per-request embedding profile registry.

use super::backend::EmbeddingRuntime;
use super::profile::{EmbeddingProfile, QueryPolicy};
use super::util::arc;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;

pub(crate) const EMBEDDING_PROFILES_ENV: &str = "RUST_CODE_MCP_EMBEDDING_PROFILES";
pub(crate) const PROJECT_PROFILE_FILE: &str = "embedding_profiles.toml";

const DEFAULT_QUERY_DOCUMENT: &str = "search_document";
const DEFAULT_QUERY_INPUT: &str = "search_query";
const DEFAULT_CHUNK_TARGET_TOKENS: usize = 768;
const DEFAULT_CHUNK_HARD_MAX_TOKENS: usize = 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileDocument {
    #[serde(default)]
    profile: Vec<TomlProfile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlProfile {
    name: String,
    #[serde(default)]
    runtime: Option<String>,
    model_id: String,
    dim: usize,
    max_len: usize,
    #[serde(default)]
    query_document: Option<String>,
    #[serde(default)]
    query_input: Option<String>,
    #[serde(default)]
    chunk_target_tokens: Option<usize>,
    #[serde(default)]
    chunk_hard_max_tokens: Option<usize>,
}

pub fn resolve_profile(
    name: &str,
    project_root: &Path,
) -> Result<EmbeddingProfile, String> {
    let user_profiles = load_user_profiles(project_root)?;
    let requested = name.to_ascii_lowercase();

    if let Some(profile) = user_profiles
        .iter()
        .find(|profile| profile.name().eq_ignore_ascii_case(requested.as_str()))
    {
        return Ok(profile.clone());
    }

    EmbeddingProfile::parse(name)
}

fn load_user_profiles(project_root: &Path) -> Result<Vec<EmbeddingProfile>, String> {
    let mut profiles = Vec::new();

    match std::env::var(EMBEDDING_PROFILES_ENV) {
        Ok(path) => {
            profiles.extend(load_profiles_from_path(Path::new(&path))?);
        }
        Err(std::env::VarError::NotPresent) => {}
        Err(err) => {
            return Err(format!(
                "failed to read {EMBEDDING_PROFILES_ENV}: {err}"
            ));
        }
    }

    let project_profile_path = project_root.join(PROJECT_PROFILE_FILE);
    if project_profile_path.exists() {
        profiles.extend(load_profiles_from_path(&project_profile_path)?);
    }

    validate_profile_names(&profiles)?;
    Ok(profiles)
}

fn load_profiles_from_path(path: &Path) -> Result<Vec<EmbeddingProfile>, String> {
    let source = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "failed to read embedding profile TOML at {}: {e}",
            path.display()
        )
    })?;
    parse_profiles_toml(&source, path)
}

fn parse_profiles_toml(
    source: &str,
    path: &Path,
) -> Result<Vec<EmbeddingProfile>, String> {
    let document: ProfileDocument = toml::from_str(source).map_err(|e| {
        format!(
            "failed to parse embedding profile TOML at {}: {e}",
            path.display()
        )
    })?;

    document
        .profile
        .into_iter()
        .map(|profile| profile.into_profile(path))
        .collect()
}

fn validate_profile_names(profiles: &[EmbeddingProfile]) -> Result<(), String> {
    let mut seen = HashSet::new();
    for profile in profiles {
        let normalized = profile.name().to_ascii_lowercase();
        if EmbeddingProfile::parse(&normalized).is_ok() {
            return Err(format!(
                "embedding profile `{}` collides with a built-in profile name or alias",
                profile.name()
            ));
        }
        if !seen.insert(normalized) {
            return Err(format!(
                "duplicate embedding profile `{}` in TOML configuration",
                profile.name()
            ));
        }
    }
    Ok(())
}

impl TomlProfile {
    fn into_profile(self, path: &Path) -> Result<EmbeddingProfile, String> {
        validate_non_empty("name", &self.name, path)?;
        validate_non_empty("model_id", &self.model_id, path)?;
        validate_positive("dim", self.dim, &self.name, path)?;
        validate_positive("max_len", self.max_len, &self.name, path)?;

        let runtime = self
            .runtime
            .as_deref()
            .unwrap_or("openrouter")
            .to_ascii_lowercase();
        if runtime != "openrouter" {
            return Err(format!(
                "embedding profile `{}` in {} uses runtime `{runtime}`; TOML profiles are API-only and must use `openrouter`",
                self.name,
                path.display()
            ));
        }

        let chunk_target_tokens = self
            .chunk_target_tokens
            .unwrap_or(DEFAULT_CHUNK_TARGET_TOKENS);
        let chunk_hard_max_tokens = self
            .chunk_hard_max_tokens
            .unwrap_or(DEFAULT_CHUNK_HARD_MAX_TOKENS);
        validate_positive(
            "chunk_target_tokens",
            chunk_target_tokens,
            &self.name,
            path,
        )?;
        validate_positive(
            "chunk_hard_max_tokens",
            chunk_hard_max_tokens,
            &self.name,
            path,
        )?;

        Ok(EmbeddingProfile {
            name: arc(&self.name),
            runtime: EmbeddingRuntime::OpenRouter,
            model_id: arc(&self.model_id),
            tokenizer_model_id: None,
            dim: self.dim,
            max_len: self.max_len,
            query_policy: QueryPolicy::InputType {
                document: arc(
                    self.query_document
                        .as_deref()
                        .unwrap_or(DEFAULT_QUERY_DOCUMENT),
                ),
                query: arc(self.query_input.as_deref().unwrap_or(DEFAULT_QUERY_INPUT)),
            },
            chunk_target_tokens,
            chunk_hard_max_tokens,
            local_loader: None,
        })
    }
}

fn validate_non_empty(field: &str, value: &str, path: &Path) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!(
            "embedding profile field `{field}` in {} must not be empty",
            path.display()
        ));
    }
    Ok(())
}

fn validate_positive(
    field: &str,
    value: usize,
    profile: &str,
    path: &Path,
) -> Result<(), String> {
    if value == 0 {
        return Err(format!(
            "embedding profile `{profile}` field `{field}` in {} must be greater than zero",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn test_path() -> PathBuf {
        PathBuf::from("test-profiles.toml")
    }

    #[test]
    fn parses_valid_openrouter_profile() {
        let profiles = parse_profiles_toml(
            r#"
[[profile]]
name = "openrouter-e5-large"
model_id = "intfloat/multilingual-e5-large"
dim = 1024
max_len = 512
query_document = "search_document"
query_input = "search_query"
chunk_target_tokens = 384
chunk_hard_max_tokens = 768
"#,
            &test_path(),
        )
        .unwrap();

        assert_eq!(profiles.len(), 1);
        let profile = &profiles[0];
        assert_eq!(profile.name(), "openrouter-e5-large");
        assert_eq!(profile.runtime, EmbeddingRuntime::OpenRouter);
        assert_eq!(profile.model_id.as_ref(), "intfloat/multilingual-e5-large");
        assert_eq!(profile.dim, 1024);
        assert_eq!(profile.max_len, 512);
        assert_eq!(
            profile.query_policy.input_types(),
            Some(("search_document", "search_query"))
        );
        assert_eq!(profile.chunk_target_tokens, 384);
        assert_eq!(profile.chunk_hard_max_tokens, 768);
        assert!(profile.local_loader.is_none());
    }

    #[test]
    fn rejects_local_runtime_profiles() {
        let err = parse_profiles_toml(
            r#"
[[profile]]
name = "bad-local"
runtime = "local-qwen3-candle-cuda"
model_id = "Qwen/Qwen3-Embedding-0.6B"
dim = 1024
max_len = 1024
"#,
            &test_path(),
        )
        .unwrap_err();

        assert!(err.contains("API-only"));
        assert!(err.contains("openrouter"));
    }

    #[test]
    fn rejects_missing_dim() {
        let err = parse_profiles_toml(
            r#"
[[profile]]
name = "missing-dim"
model_id = "provider/model"
max_len = 512
"#,
            &test_path(),
        )
        .unwrap_err();

        assert!(err.contains("missing field `dim`"));
    }

    #[test]
    fn rejects_unknown_fields_including_credentials() {
        let err = parse_profiles_toml(
            r#"
[[profile]]
name = "bad-secret"
model_id = "provider/model"
dim = 1024
max_len = 512
api_key = "do-not-store"
"#,
            &test_path(),
        )
        .unwrap_err();

        assert!(err.contains("unknown field `api_key`"));
    }

    #[test]
    fn rejects_built_in_name_collisions() {
        let profiles = parse_profiles_toml(
            r#"
[[profile]]
name = "local-gpu-small"
model_id = "provider/model"
dim = 1024
max_len = 512
"#,
            &test_path(),
        )
        .unwrap();
        let err = validate_profile_names(&profiles).unwrap_err();

        assert!(err.contains("collides with a built-in"));
    }

    #[test]
    fn rejects_duplicate_user_names() {
        let profiles = parse_profiles_toml(
            r#"
[[profile]]
name = "same"
model_id = "provider/one"
dim = 1024
max_len = 512

[[profile]]
name = "SAME"
model_id = "provider/two"
dim = 1024
max_len = 512
"#,
            &test_path(),
        )
        .unwrap();
        let err = validate_profile_names(&profiles).unwrap_err();

        assert!(err.contains("duplicate embedding profile"));
    }

    #[test]
    fn bad_path_error_is_clear() {
        let err = load_profiles_from_path(Path::new("/definitely/missing.toml"))
            .unwrap_err();

        assert!(err.contains("failed to read embedding profile TOML"));
    }

    #[test]
    fn resolve_profile_reads_project_root_toml() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join(PROJECT_PROFILE_FILE),
            r#"
[[profile]]
name = "project-openrouter"
model_id = "provider/project"
dim = 2048
max_len = 1024
"#,
        )
        .unwrap();

        let built_in = resolve_profile("local-gpu-small", dir.path()).unwrap();
        let alias = resolve_profile("bge-small-cpu", dir.path()).unwrap();
        let dynamic = resolve_profile("project-openrouter", dir.path()).unwrap();

        assert_eq!(built_in.name(), "local-gpu-small");
        assert_eq!(alias.name(), "local-cpu-small");
        assert_eq!(dynamic.model_id.as_ref(), "provider/project");
        assert_eq!(dynamic.dim, 2048);
    }
}
