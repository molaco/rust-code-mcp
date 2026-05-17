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
const MAX_RETRIES: usize = 3;

#[derive(Clone)]
pub(super) struct OpenRouterEmbedder {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
    backend: EmbeddingBackend,
    dim: usize,
}

#[derive(Debug, Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
    encoding_format: &'static str,
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

        Ok(Self {
            client,
            api_key,
            base_url,
            model,
            backend: *backend,
            dim: backend.dim(),
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
        let formatted: Vec<String> = texts
            .iter()
            .map(|text| self.backend.format_query(text))
            .collect();
        self.embed_with_split(formatted, "search_query").await
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
            encoding_format: "float",
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
}
