//! Runtime configuration and env-var parsing for the OpenRouter backend.

use crate::embeddings::EmbeddingError;
use serde::Serialize;

pub(super) const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1/embeddings";
pub(super) const API_KEY_ENV: &str = "RUST_CODE_MCP_OPENROUTER_API_KEY";
pub(super) const FALLBACK_API_KEY_ENV: &str = "OPENROUTER_API_KEY";
pub(super) const BASE_URL_ENV: &str = "RUST_CODE_MCP_OPENROUTER_BASE_URL";
pub(super) const MAX_BATCH_INPUTS_ENV: &str = "RUST_CODE_MCP_OPENROUTER_MAX_BATCH_INPUTS";
pub(super) const MAX_BATCH_TOKENS_ENV: &str = "RUST_CODE_MCP_OPENROUTER_MAX_BATCH_TOKENS";
pub(super) const CONCURRENCY_ENV: &str = "RUST_CODE_MCP_OPENROUTER_CONCURRENCY";
pub(super) const ENCODING_FORMAT_ENV: &str = "RUST_CODE_MCP_OPENROUTER_ENCODING_FORMAT";
pub(super) const PROVIDER_SORT_ENV: &str = "RUST_CODE_MCP_OPENROUTER_PROVIDER_SORT";
pub(super) const PROVIDER_MIN_THROUGHPUT_ENV: &str =
    "RUST_CODE_MCP_OPENROUTER_PREFERRED_MIN_THROUGHPUT";
pub(super) const PROVIDER_MAX_LATENCY_ENV: &str =
    "RUST_CODE_MCP_OPENROUTER_PREFERRED_MAX_LATENCY";
pub(super) const DEFAULT_MAX_BATCH_INPUTS: usize = 128;
pub(super) const DEFAULT_MAX_BATCH_TOKENS: usize = 131_072;
pub(super) const DEFAULT_CONCURRENCY: usize = 4;
pub(super) const MAX_BATCH_INPUTS: usize = 512;
pub(super) const MAX_BATCH_TOKENS: usize = 1_048_576;
pub(super) const MAX_CONCURRENCY: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OpenRouterRuntimeConfig {
    pub max_batch_inputs: usize,
    pub max_batch_tokens: usize,
    pub concurrency: usize,
    pub encoding_format: OpenRouterEncodingFormat,
    pub provider: Option<OpenRouterProviderPreferences>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenRouterEncodingFormat {
    Float,
    Base64,
}

impl OpenRouterEncodingFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Float => "float",
            Self::Base64 => "base64",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct OpenRouterProviderPreferences {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<OpenRouterProviderSort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_min_throughput: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_max_latency: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenRouterProviderSort {
    Price,
    Throughput,
    Latency,
}

impl OpenRouterProviderSort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Price => "price",
            Self::Throughput => "throughput",
            Self::Latency => "latency",
        }
    }
}

pub(super) fn api_key_from_env() -> Result<String, EmbeddingError> {
    resolve_api_key(|key| std::env::var(key))
}

pub fn openrouter_runtime_config() -> OpenRouterRuntimeConfig {
    openrouter_runtime_config_from_env()
}

pub(super) fn openrouter_runtime_config_from_env() -> OpenRouterRuntimeConfig {
    resolve_openrouter_runtime_config(|key| std::env::var(key))
}

pub(super) fn resolve_openrouter_runtime_config<F>(mut get_var: F) -> OpenRouterRuntimeConfig
where
    F: FnMut(&str) -> Result<String, std::env::VarError>,
{
    OpenRouterRuntimeConfig {
        max_batch_inputs: positive_usize_from_env(
            &mut get_var,
            MAX_BATCH_INPUTS_ENV,
            DEFAULT_MAX_BATCH_INPUTS,
            MAX_BATCH_INPUTS,
            "OpenRouter max batch input count",
        ),
        max_batch_tokens: positive_usize_from_env(
            &mut get_var,
            MAX_BATCH_TOKENS_ENV,
            DEFAULT_MAX_BATCH_TOKENS,
            MAX_BATCH_TOKENS,
            "OpenRouter max batch token budget",
        ),
        concurrency: positive_usize_from_env(
            &mut get_var,
            CONCURRENCY_ENV,
            DEFAULT_CONCURRENCY,
            MAX_CONCURRENCY,
            "OpenRouter concurrency",
        ),
        encoding_format: encoding_format_from_env(&mut get_var),
        provider: provider_preferences_from_env(&mut get_var),
    }
}

fn positive_usize_from_env<F>(
    get_var: &mut F,
    env_var: &'static str,
    default: usize,
    max: usize,
    label: &'static str,
) -> usize
where
    F: FnMut(&str) -> Result<String, std::env::VarError>,
{
    let raw = match get_var(env_var) {
        Ok(raw) => raw,
        Err(std::env::VarError::NotPresent) => return default,
        Err(err) => {
            tracing::warn!(
                env_var,
                error = ?err,
                default,
                "Ignoring unreadable {label} override"
            );
            return default;
        }
    };

    let parsed = match raw.trim().parse::<usize>() {
        Ok(parsed) if parsed > 0 => parsed,
        Ok(_) => {
            tracing::warn!(
                env_var,
                value = raw.as_str(),
                default,
                "Ignoring invalid {label} override; value must be greater than zero"
            );
            return default;
        }
        Err(_) => {
            tracing::warn!(
                env_var,
                value = raw.as_str(),
                default,
                "Ignoring invalid {label} override; value must be a positive integer"
            );
            return default;
        }
    };

    if parsed > max {
        tracing::warn!(
            env_var,
            requested = parsed,
            max,
            "Clamping {label} override"
        );
    }

    parsed.min(max)
}

fn encoding_format_from_env<F>(get_var: &mut F) -> OpenRouterEncodingFormat
where
    F: FnMut(&str) -> Result<String, std::env::VarError>,
{
    let raw = match get_var(ENCODING_FORMAT_ENV) {
        Ok(raw) => raw,
        Err(std::env::VarError::NotPresent) => return OpenRouterEncodingFormat::Float,
        Err(err) => {
            tracing::warn!(
                env_var = ENCODING_FORMAT_ENV,
                error = ?err,
                default = OpenRouterEncodingFormat::Float.as_str(),
                "Ignoring unreadable OpenRouter encoding format override"
            );
            return OpenRouterEncodingFormat::Float;
        }
    };

    match raw.trim().to_ascii_lowercase().as_str() {
        "float" => OpenRouterEncodingFormat::Float,
        "base64" => OpenRouterEncodingFormat::Base64,
        _ => {
            tracing::warn!(
                env_var = ENCODING_FORMAT_ENV,
                value = raw.as_str(),
                default = OpenRouterEncodingFormat::Float.as_str(),
                "Ignoring unsupported OpenRouter encoding format override"
            );
            OpenRouterEncodingFormat::Float
        }
    }
}

fn provider_preferences_from_env<F>(
    get_var: &mut F,
) -> Option<OpenRouterProviderPreferences>
where
    F: FnMut(&str) -> Result<String, std::env::VarError>,
{
    let sort = provider_sort_from_env(get_var);
    let preferred_min_throughput = optional_usize_from_env(
        get_var,
        PROVIDER_MIN_THROUGHPUT_ENV,
        "OpenRouter provider preferred minimum throughput",
    );
    let preferred_max_latency = optional_f64_from_env(
        get_var,
        PROVIDER_MAX_LATENCY_ENV,
        "OpenRouter provider preferred maximum latency",
    );

    if sort.is_none()
        && preferred_min_throughput.is_none()
        && preferred_max_latency.is_none()
    {
        None
    } else {
        Some(OpenRouterProviderPreferences {
            sort,
            preferred_min_throughput,
            preferred_max_latency,
        })
    }
}

fn provider_sort_from_env<F>(get_var: &mut F) -> Option<OpenRouterProviderSort>
where
    F: FnMut(&str) -> Result<String, std::env::VarError>,
{
    let raw = match get_var(PROVIDER_SORT_ENV) {
        Ok(raw) => raw,
        Err(std::env::VarError::NotPresent) => return None,
        Err(err) => {
            tracing::warn!(
                env_var = PROVIDER_SORT_ENV,
                error = ?err,
                "Ignoring unreadable OpenRouter provider sort override"
            );
            return None;
        }
    };

    match raw.trim().to_ascii_lowercase().as_str() {
        "price" => Some(OpenRouterProviderSort::Price),
        "throughput" => Some(OpenRouterProviderSort::Throughput),
        "latency" => Some(OpenRouterProviderSort::Latency),
        _ => {
            tracing::warn!(
                env_var = PROVIDER_SORT_ENV,
                value = raw.as_str(),
                "Ignoring unsupported OpenRouter provider sort override"
            );
            None
        }
    }
}

fn optional_usize_from_env<F>(
    get_var: &mut F,
    env_var: &'static str,
    label: &'static str,
) -> Option<usize>
where
    F: FnMut(&str) -> Result<String, std::env::VarError>,
{
    let raw = match get_var(env_var) {
        Ok(raw) => raw,
        Err(std::env::VarError::NotPresent) => return None,
        Err(err) => {
            tracing::warn!(
                env_var,
                error = ?err,
                "Ignoring unreadable {label} override"
            );
            return None;
        }
    };

    match raw.trim().parse::<usize>() {
        Ok(value) if value > 0 => Some(value),
        Ok(_) => {
            tracing::warn!(
                env_var,
                value = raw.as_str(),
                "Ignoring invalid {label} override; value must be greater than zero"
            );
            None
        }
        Err(_) => {
            tracing::warn!(
                env_var,
                value = raw.as_str(),
                "Ignoring invalid {label} override; value must be a positive integer"
            );
            None
        }
    }
}

fn optional_f64_from_env<F>(
    get_var: &mut F,
    env_var: &'static str,
    label: &'static str,
) -> Option<f64>
where
    F: FnMut(&str) -> Result<String, std::env::VarError>,
{
    let raw = match get_var(env_var) {
        Ok(raw) => raw,
        Err(std::env::VarError::NotPresent) => return None,
        Err(err) => {
            tracing::warn!(
                env_var,
                error = ?err,
                "Ignoring unreadable {label} override"
            );
            return None;
        }
    };

    match raw.trim().parse::<f64>() {
        Ok(value) if value.is_finite() && value > 0.0 => Some(value),
        Ok(_) => {
            tracing::warn!(
                env_var,
                value = raw.as_str(),
                "Ignoring invalid {label} override; value must be finite and greater than zero"
            );
            None
        }
        Err(_) => {
            tracing::warn!(
                env_var,
                value = raw.as_str(),
                "Ignoring invalid {label} override; value must be a positive number"
            );
            None
        }
    }
}

pub(super) fn resolve_api_key<F>(mut get_var: F) -> Result<String, EmbeddingError>
where
    F: FnMut(&str) -> Result<String, std::env::VarError>,
{
    get_var(API_KEY_ENV)
        .or_else(|_| get_var(FALLBACK_API_KEY_ENV))
        .map(|key| key.trim().to_string())
        .ok()
        .filter(|key| !key.is_empty())
        .ok_or_else(|| {
            EmbeddingError::model_init(format!(
                "missing OpenRouter API key; set {API_KEY_ENV} or {FALLBACK_API_KEY_ENV}"
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_api_key_is_clear() {
        let err = resolve_api_key(|_| Err(std::env::VarError::NotPresent)).unwrap_err();

        assert!(err.to_string().contains("missing OpenRouter API key"));
        assert!(err.to_string().contains(API_KEY_ENV));
    }

    #[test]
    fn runtime_config_uses_defaults() {
        let config = config_from_pairs(&[]);

        assert_eq!(config.max_batch_inputs, DEFAULT_MAX_BATCH_INPUTS);
        assert_eq!(config.max_batch_tokens, DEFAULT_MAX_BATCH_TOKENS);
        assert_eq!(config.concurrency, DEFAULT_CONCURRENCY);
        assert_eq!(config.encoding_format, OpenRouterEncodingFormat::Float);
        assert_eq!(config.provider, None);
    }

    #[test]
    fn runtime_config_accepts_valid_overrides() {
        let config = config_from_pairs(&[
            (MAX_BATCH_INPUTS_ENV, "64"),
            (MAX_BATCH_TOKENS_ENV, "65536"),
            (CONCURRENCY_ENV, "8"),
            (ENCODING_FORMAT_ENV, "float"),
            (PROVIDER_SORT_ENV, "throughput"),
            (PROVIDER_MIN_THROUGHPUT_ENV, "5000"),
            (PROVIDER_MAX_LATENCY_ENV, "2.5"),
        ]);

        assert_eq!(config.max_batch_inputs, 64);
        assert_eq!(config.max_batch_tokens, 65_536);
        assert_eq!(config.concurrency, 8);
        assert_eq!(config.encoding_format.as_str(), "float");
        assert_eq!(
            config.provider,
            Some(OpenRouterProviderPreferences {
                sort: Some(OpenRouterProviderSort::Throughput),
                preferred_min_throughput: Some(5000),
                preferred_max_latency: Some(2.5),
            })
        );
    }

    #[test]
    fn runtime_config_rejects_zero_and_invalid_overrides() {
        let config = config_from_pairs(&[
            (MAX_BATCH_INPUTS_ENV, "0"),
            (MAX_BATCH_TOKENS_ENV, "abc"),
            (CONCURRENCY_ENV, ""),
            (ENCODING_FORMAT_ENV, "xml"),
            (PROVIDER_SORT_ENV, "fastest"),
            (PROVIDER_MIN_THROUGHPUT_ENV, "0"),
            (PROVIDER_MAX_LATENCY_ENV, "nan"),
        ]);

        assert_eq!(config.max_batch_inputs, DEFAULT_MAX_BATCH_INPUTS);
        assert_eq!(config.max_batch_tokens, DEFAULT_MAX_BATCH_TOKENS);
        assert_eq!(config.concurrency, DEFAULT_CONCURRENCY);
        assert_eq!(config.encoding_format, OpenRouterEncodingFormat::Float);
        assert_eq!(config.provider, None);
    }

    #[test]
    fn runtime_config_accepts_base64_encoding() {
        let config = config_from_pairs(&[(ENCODING_FORMAT_ENV, "base64")]);

        assert_eq!(config.encoding_format, OpenRouterEncodingFormat::Base64);
        assert_eq!(config.encoding_format.as_str(), "base64");
    }

    #[test]
    fn runtime_config_clamps_large_overrides() {
        let config = config_from_pairs(&[
            (MAX_BATCH_INPUTS_ENV, "999999"),
            (MAX_BATCH_TOKENS_ENV, "999999999"),
            (CONCURRENCY_ENV, "999"),
        ]);

        assert_eq!(config.max_batch_inputs, MAX_BATCH_INPUTS);
        assert_eq!(config.max_batch_tokens, MAX_BATCH_TOKENS);
        assert_eq!(config.concurrency, MAX_CONCURRENCY);
    }

    #[test]
    fn provider_preferences_are_optional_and_partial() {
        let config = config_from_pairs(&[
            (PROVIDER_SORT_ENV, "latency"),
            (PROVIDER_MAX_LATENCY_ENV, "1.25"),
        ]);

        assert_eq!(
            config.provider,
            Some(OpenRouterProviderPreferences {
                sort: Some(OpenRouterProviderSort::Latency),
                preferred_min_throughput: None,
                preferred_max_latency: Some(1.25),
            })
        );
    }

    fn config_from_pairs(pairs: &[(&str, &str)]) -> OpenRouterRuntimeConfig {
        resolve_openrouter_runtime_config(|key| {
            pairs
                .iter()
                .find_map(|(pair_key, value)| {
                    if *pair_key == key {
                        Some((*value).to_string())
                    } else {
                        None
                    }
                })
                .ok_or(std::env::VarError::NotPresent)
        })
    }
}
