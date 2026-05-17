//! OpenRouter embeddings backend.

use crate::embeddings::backend::{EmbeddingBackend, EmbeddingRuntime};
use crate::embeddings::{Embedding, EmbeddingError};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::Duration;

const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1/embeddings";
const API_KEY_ENV: &str = "RUST_CODE_MCP_OPENROUTER_API_KEY";
const FALLBACK_API_KEY_ENV: &str = "OPENROUTER_API_KEY";
const BASE_URL_ENV: &str = "RUST_CODE_MCP_OPENROUTER_BASE_URL";
const MAX_BATCH_INPUTS_ENV: &str = "RUST_CODE_MCP_OPENROUTER_MAX_BATCH_INPUTS";
const MAX_BATCH_TOKENS_ENV: &str = "RUST_CODE_MCP_OPENROUTER_MAX_BATCH_TOKENS";
const CONCURRENCY_ENV: &str = "RUST_CODE_MCP_OPENROUTER_CONCURRENCY";
const ENCODING_FORMAT_ENV: &str = "RUST_CODE_MCP_OPENROUTER_ENCODING_FORMAT";
const MAX_RETRIES: usize = 3;
const DEFAULT_MAX_BATCH_INPUTS: usize = 128;
const DEFAULT_MAX_BATCH_TOKENS: usize = 131_072;
const DEFAULT_CONCURRENCY: usize = 4;
const MAX_BATCH_INPUTS: usize = 512;
const MAX_BATCH_TOKENS: usize = 1_048_576;
const MAX_CONCURRENCY: usize = 16;

#[derive(Clone)]
pub(super) struct OpenRouterEmbedder {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
    dim: usize,
    config: OpenRouterRuntimeConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OpenRouterRuntimeConfig {
    pub(crate) max_batch_inputs: usize,
    pub(crate) max_batch_tokens: usize,
    pub(crate) concurrency: usize,
    pub(crate) encoding_format: OpenRouterEncodingFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenRouterEncodingFormat {
    Float,
}

impl OpenRouterEncodingFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::Float => "float",
        }
    }
}

#[derive(Debug, Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
    encoding_format: &'a str,
    dimensions: usize,
    input_type: &'a str,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingResponseItem>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponseItem {
    embedding: Vec<f32>,
    index: usize,
}

#[derive(Debug)]
enum OpenRouterBatchError {
    PayloadTooLarge(String),
    Fatal(String),
}

impl OpenRouterEmbedder {
    pub(super) fn new(backend: &EmbeddingBackend) -> Result<Self, EmbeddingError> {
        if backend.runtime != EmbeddingRuntime::OpenRouter {
            return Err(EmbeddingError::model_init(format!(
                "embedding profile `{}` is not an OpenRouter profile",
                backend.profile.name()
            )));
        }

        let api_key = api_key_from_env()?;
        let base_url = std::env::var(BASE_URL_ENV)
            .unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();
        let model = backend
            .model
            .openrouter_model_id()
            .ok_or_else(|| {
                EmbeddingError::model_init(format!(
                    "embedding model `{}` is not available through OpenRouter",
                    backend.model.display_name()
                ))
            })?
            .to_string();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| EmbeddingError::model_init(e.to_string()))?;
        let config = openrouter_runtime_config_from_env();

        tracing::info!(
            max_batch_inputs = config.max_batch_inputs,
            max_batch_tokens = config.max_batch_tokens,
            concurrency = config.concurrency,
            encoding_format = config.encoding_format.as_str(),
            "OpenRouter embedding runtime configured"
        );

        Ok(Self {
            client,
            api_key,
            base_url,
            model,
            dim: backend.dim(),
            config,
        })
    }

    pub(super) fn dim(&self) -> usize {
        self.dim
    }

    pub(super) async fn embed_documents(
        &self,
        texts: Vec<String>,
    ) -> Result<Vec<Embedding>, EmbeddingError> {
        self.embed_with_split(texts, "search_document").await
    }

    pub(super) async fn embed_queries(
        &self,
        texts: Vec<String>,
    ) -> Result<Vec<Embedding>, EmbeddingError> {
        self.embed_with_split(texts, "search_query").await
    }

    async fn embed_with_split(
        &self,
        texts: Vec<String>,
        input_type: &str,
    ) -> Result<Vec<Embedding>, EmbeddingError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut output: Vec<Option<Embedding>> = vec![None; texts.len()];
        let mut pending = VecDeque::from([(0usize, texts)]);

        while let Some((start, batch)) = pending.pop_front() {
            match self.request_batch(&batch, input_type).await {
                Ok(embeddings) => {
                    for (offset, embedding) in embeddings.into_iter().enumerate() {
                        output[start + offset] = Some(embedding);
                    }
                }
                Err(OpenRouterBatchError::PayloadTooLarge(msg)) if batch.len() > 1 => {
                    let mid = batch.len() / 2;
                    let right = batch[mid..].to_vec();
                    let left = batch[..mid].to_vec();
                    pending.push_front((start + mid, right));
                    pending.push_front((start, left));
                    tracing::warn!(
                        batch_len = batch.len(),
                        "OpenRouter embedding batch was too large; splitting batch: {}",
                        msg
                    );
                }
                Err(OpenRouterBatchError::PayloadTooLarge(msg)) => {
                    return Err(EmbeddingError::embed_failed(format!(
                        "OpenRouter rejected a single embedding input as too large: {msg}"
                    )));
                }
                Err(OpenRouterBatchError::Fatal(msg)) => {
                    return Err(EmbeddingError::embed_failed(msg));
                }
            }
        }

        output
            .into_iter()
            .enumerate()
            .map(|(idx, maybe)| {
                maybe.ok_or_else(|| {
                    EmbeddingError::embed_failed(format!(
                        "OpenRouter response did not include embedding at index {idx}"
                    ))
                })
            })
            .collect()
    }

    async fn request_batch(
        &self,
        texts: &[String],
        input_type: &str,
    ) -> Result<Vec<Embedding>, OpenRouterBatchError> {
        let request = EmbeddingRequest {
            model: &self.model,
            input: texts,
            encoding_format: self.config.encoding_format.as_str(),
            dimensions: self.dim,
            input_type,
        };

        let mut last_retryable = None;
        for attempt in 0..=MAX_RETRIES {
            let response = self
                .client
                .post(&self.base_url)
                .bearer_auth(&self.api_key)
                .header("Content-Type", "application/json")
                .json(&request)
                .send()
                .await;

            match response {
                Ok(response) if response.status().is_success() => {
                    let status = response.status();
                    let body = response.text().await.map_err(|e| {
                        OpenRouterBatchError::Fatal(format!(
                            "OpenRouter response body read failed after {status}: {e}"
                        ))
                    })?;
                    return parse_embeddings_response(&body, self.dim, texts.len())
                        .map_err(OpenRouterBatchError::Fatal);
                }
                Ok(response) => {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    let msg = format!(
                        "OpenRouter embeddings request failed with HTTP {status}: {}",
                        body_snippet(&body)
                    );
                    if is_payload_too_large(status, &body) {
                        return Err(OpenRouterBatchError::PayloadTooLarge(msg));
                    }
                    if is_retryable_status(status) && attempt < MAX_RETRIES {
                        last_retryable = Some(msg);
                        sleep_for_retry(attempt).await;
                        continue;
                    }
                    return Err(OpenRouterBatchError::Fatal(msg));
                }
                Err(err) if is_retryable_reqwest_error(&err) && attempt < MAX_RETRIES => {
                    last_retryable = Some(format!("OpenRouter request failed: {err}"));
                    sleep_for_retry(attempt).await;
                    continue;
                }
                Err(err) => {
                    return Err(OpenRouterBatchError::Fatal(format!(
                        "OpenRouter request failed: {err}"
                    )));
                }
            }
        }

        Err(OpenRouterBatchError::Fatal(
            last_retryable.unwrap_or_else(|| "OpenRouter request failed".to_string()),
        ))
    }
}

fn api_key_from_env() -> Result<String, EmbeddingError> {
    resolve_api_key(|key| std::env::var(key))
}

fn openrouter_runtime_config_from_env() -> OpenRouterRuntimeConfig {
    resolve_openrouter_runtime_config(|key| std::env::var(key))
}

fn resolve_openrouter_runtime_config<F>(mut get_var: F) -> OpenRouterRuntimeConfig
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

fn resolve_api_key<F>(mut get_var: F) -> Result<String, EmbeddingError>
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

fn parse_embeddings_response(
    body: &str,
    expected_dim: usize,
    expected_count: usize,
) -> Result<Vec<Embedding>, String> {
    let response: EmbeddingResponse = serde_json::from_str(body)
        .map_err(|e| format!("OpenRouter embeddings response was not valid JSON: {e}"))?;
    if response.data.len() != expected_count {
        return Err(format!(
            "OpenRouter returned {} embeddings for {} inputs",
            response.data.len(),
            expected_count
        ));
    }

    let mut output: Vec<Option<Embedding>> = vec![None; expected_count];
    for item in response.data {
        if item.index >= expected_count {
            return Err(format!(
                "OpenRouter returned out-of-range embedding index {} for {} inputs",
                item.index, expected_count
            ));
        }
        if item.embedding.len() != expected_dim {
            return Err(format!(
                "OpenRouter returned embedding dimension {} at index {}, expected {}",
                item.embedding.len(),
                item.index,
                expected_dim
            ));
        }
        output[item.index] = Some(item.embedding);
    }

    output
        .into_iter()
        .enumerate()
        .map(|(idx, maybe)| {
            maybe.ok_or_else(|| {
                format!("OpenRouter response omitted embedding for input index {idx}")
            })
        })
        .collect()
}

fn body_snippet(body: &str) -> String {
    let mut snippet: String = body.chars().take(500).collect();
    if body.chars().count() > 500 {
        snippet.push_str("...");
    }
    snippet
}

fn is_payload_too_large(status: StatusCode, body: &str) -> bool {
    status == StatusCode::PAYLOAD_TOO_LARGE
        || (status == StatusCode::BAD_REQUEST
            && body.to_ascii_lowercase().contains("too large"))
}

fn is_retryable_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::INTERNAL_SERVER_ERROR
        || status == StatusCode::BAD_GATEWAY
        || status == StatusCode::SERVICE_UNAVAILABLE
        || status == StatusCode::GATEWAY_TIMEOUT
        || status.as_u16() == 529
}

fn is_retryable_reqwest_error(err: &reqwest::Error) -> bool {
    err.is_connect() || err.is_timeout()
}

async fn sleep_for_retry(attempt: usize) {
    let delay_ms = 250u64.saturating_mul(1u64 << attempt.min(4));
    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_embeddings_response_in_index_order() {
        let body = r#"{
            "data": [
                {"embedding": [3.0, 4.0], "index": 1},
                {"embedding": [1.0, 2.0], "index": 0}
            ],
            "model": "qwen/qwen3-embedding-8b",
            "object": "list"
        }"#;

        let embeddings = parse_embeddings_response(body, 2, 2).unwrap();

        assert_eq!(embeddings, vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
    }

    #[test]
    fn rejects_dimension_mismatch() {
        let body = r#"{
            "data": [
                {"embedding": [1.0], "index": 0}
            ]
        }"#;

        let err = parse_embeddings_response(body, 2, 1).unwrap_err();

        assert!(err.contains("embedding dimension 1"));
    }

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
    }

    #[test]
    fn runtime_config_accepts_valid_overrides() {
        let config = config_from_pairs(&[
            (MAX_BATCH_INPUTS_ENV, "64"),
            (MAX_BATCH_TOKENS_ENV, "65536"),
            (CONCURRENCY_ENV, "8"),
            (ENCODING_FORMAT_ENV, "float"),
        ]);

        assert_eq!(config.max_batch_inputs, 64);
        assert_eq!(config.max_batch_tokens, 65_536);
        assert_eq!(config.concurrency, 8);
        assert_eq!(config.encoding_format.as_str(), "float");
    }

    #[test]
    fn runtime_config_rejects_zero_and_invalid_overrides() {
        let config = config_from_pairs(&[
            (MAX_BATCH_INPUTS_ENV, "0"),
            (MAX_BATCH_TOKENS_ENV, "abc"),
            (CONCURRENCY_ENV, ""),
            (ENCODING_FORMAT_ENV, "base64"),
        ]);

        assert_eq!(config.max_batch_inputs, DEFAULT_MAX_BATCH_INPUTS);
        assert_eq!(config.max_batch_tokens, DEFAULT_MAX_BATCH_TOKENS);
        assert_eq!(config.concurrency, DEFAULT_CONCURRENCY);
        assert_eq!(config.encoding_format, OpenRouterEncodingFormat::Float);
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
