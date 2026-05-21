//! HTTP orchestration and the `OpenRouterEmbedder` embedder type.

use crate::embeddings::backend::{EmbeddingBackend, EmbeddingRuntime};
use crate::embeddings::openrouter::batch::{
    fallback_token_estimate, plan_remote_input_batches, restore_original_embedding_order,
    OpenRouterBatchError, OpenRouterInputBatch,
};
use crate::embeddings::openrouter::config::{
    api_key_from_env, openrouter_runtime_config_from_env, OpenRouterRuntimeConfig, BASE_URL_ENV,
    DEFAULT_BASE_URL,
};
use crate::embeddings::openrouter::metrics::{
    log_openrouter_request_metrics, OpenRouterMetricsHandle, OpenRouterRequestMetrics,
};
use crate::embeddings::openrouter::request::EmbeddingRequest;
use crate::embeddings::openrouter::response::parse_embeddings_response;
use crate::embeddings::openrouter::retry::{
    body_snippet, is_payload_too_large, is_retryable_reqwest_error, is_retryable_status,
    sleep_for_retry, MAX_RETRIES,
};
use crate::embeddings::{Embedding, EmbeddingError, EmbeddingTokenCounter};
use futures::stream::{self, StreamExt};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(in crate::embeddings) struct OpenRouterEmbedder {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
    dim: usize,
    document_input_type: String,
    query_input_type: String,
    config: OpenRouterRuntimeConfig,
    token_counter: Option<EmbeddingTokenCounter>,
}

impl OpenRouterEmbedder {
    pub(in crate::embeddings) fn new(backend: &EmbeddingBackend) -> Result<Self, EmbeddingError> {
        if backend.runtime != EmbeddingRuntime::OpenRouter {
            return Err(EmbeddingError::model_init(format!(
                "embedding profile `{}` is not an OpenRouter profile",
                backend.profile.name()
            )));
        }

        let api_key = api_key_from_env()?;
        let (document_input_type, query_input_type) = backend
            .profile
            .query_policy
            .input_types()
            .ok_or_else(|| {
                EmbeddingError::model_init(format!(
                    "embedding profile `{}` does not define OpenRouter input_type values",
                    backend.profile.name()
                ))
            })?;
        let base_url = std::env::var(BASE_URL_ENV)
            .unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();
        let model = backend.model_id().to_string();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| EmbeddingError::model_init(e.to_string()))?;
        let config = openrouter_runtime_config_from_env();
        let token_counter = match EmbeddingTokenCounter::from_backend(backend) {
            Ok(counter) => {
                tracing::info!(
                    max_len = counter.max_len(),
                    "OpenRouter token counter initialized"
                );
                Some(counter)
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "OpenRouter token counter unavailable; remote batches will use text-length estimates"
                );
                None
            }
        };

        tracing::info!(
            max_batch_inputs = config.max_batch_inputs,
            max_batch_tokens = config.max_batch_tokens,
            concurrency = config.concurrency,
            encoding_format = config.encoding_format.as_str(),
            provider_preferences = config.provider.is_some(),
            "OpenRouter embedding runtime configured"
        );

        Ok(Self {
            client,
            api_key,
            base_url,
            model,
            dim: backend.dim(),
            document_input_type: document_input_type.to_string(),
            query_input_type: query_input_type.to_string(),
            config,
            token_counter,
        })
    }

    pub(in crate::embeddings) fn dim(&self) -> usize {
        self.dim
    }

    pub(in crate::embeddings) async fn embed_documents(
        &self,
        texts: Vec<String>,
    ) -> Result<Vec<Embedding>, EmbeddingError> {
        self.embed_with_split(texts, &self.document_input_type).await
    }

    pub(in crate::embeddings) async fn embed_queries(
        &self,
        texts: Vec<String>,
    ) -> Result<Vec<Embedding>, EmbeddingError> {
        self.embed_with_split(texts, &self.query_input_type).await
    }

    fn plan_remote_batches(&self, texts: Vec<String>) -> Vec<OpenRouterInputBatch> {
        let token_lengths = self.estimate_token_lengths(&texts);
        plan_remote_input_batches(texts, token_lengths, self.config)
    }

    fn estimate_token_lengths(&self, texts: &[String]) -> Vec<usize> {
        if let Some(counter) = self.token_counter.as_ref() {
            match counter.count_batch(texts) {
                Ok(lengths) => {
                    return lengths
                        .into_iter()
                        .map(|len| len.capped_tokens.max(1))
                        .collect();
                }
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "OpenRouter token counting failed; remote batches will use text-length estimates"
                    );
                }
            }
        }

        texts
            .iter()
            .map(|text| fallback_token_estimate(text).max(1))
            .collect()
    }

    async fn embed_with_split(
        &self,
        texts: Vec<String>,
        input_type: &str,
    ) -> Result<Vec<Embedding>, EmbeddingError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let expected_count = texts.len();
        let batches = self.plan_remote_batches(texts);
        let embed_start = Instant::now();
        let metrics = Arc::new(Mutex::new(OpenRouterRequestMetrics::default()));
        tracing::info!(
            inputs = expected_count,
            request_batches = batches.len(),
            concurrency = self.config.concurrency,
            max_batch_inputs = self.config.max_batch_inputs,
            max_batch_tokens = self.config.max_batch_tokens,
            "OpenRouter embedding request plan"
        );

        let mut request_stream = stream::iter(batches)
            .map(|batch| {
                self.request_batch_with_split(batch, input_type, metrics.clone())
            })
            .buffer_unordered(self.config.concurrency);
        let mut ordered_embeddings = Vec::with_capacity(expected_count);

        while let Some(result) = request_stream.next().await {
            match result {
                Ok(mut embeddings) => ordered_embeddings.append(&mut embeddings),
                Err(OpenRouterBatchError::PayloadTooLarge(msg)) => {
                    log_openrouter_request_metrics(
                        &metrics.lock().unwrap(),
                        embed_start.elapsed(),
                        ordered_embeddings.len(),
                    );
                    return Err(EmbeddingError::embed_failed(format!(
                        "OpenRouter rejected a single embedding input as too large: {msg}"
                    )));
                }
                Err(OpenRouterBatchError::Fatal(msg)) => {
                    log_openrouter_request_metrics(
                        &metrics.lock().unwrap(),
                        embed_start.elapsed(),
                        ordered_embeddings.len(),
                    );
                    return Err(EmbeddingError::embed_failed(msg));
                }
            }
        }

        log_openrouter_request_metrics(
            &metrics.lock().unwrap(),
            embed_start.elapsed(),
            ordered_embeddings.len(),
        );

        restore_original_embedding_order(expected_count, ordered_embeddings)
            .map_err(EmbeddingError::embed_failed)
    }

    async fn request_batch_with_split(
        &self,
        batch: OpenRouterInputBatch,
        input_type: &str,
        metrics: OpenRouterMetricsHandle,
    ) -> Result<Vec<(usize, Embedding)>, OpenRouterBatchError> {
        let mut ordered_embeddings = Vec::with_capacity(batch.len());
        let mut pending = VecDeque::from([batch]);

        while let Some(batch) = pending.pop_front() {
            let texts = batch.texts();
            let estimated_tokens = batch.padded_tokens();
            match self
                .request_batch(&texts, input_type, estimated_tokens, metrics.clone())
                .await
            {
                Ok(embeddings) => {
                    ordered_embeddings.extend(
                        batch
                            .inputs
                            .into_iter()
                            .zip(embeddings)
                            .map(|(input, embedding)| (input.original_index, embedding)),
                    );
                }
                Err(OpenRouterBatchError::PayloadTooLarge(msg)) if batch.len() > 1 => {
                    let batch_len = batch.len();
                    let (left, right) = batch.split_at(batch_len / 2);
                    metrics.lock().unwrap().record_split();
                    pending.push_front(right);
                    pending.push_front(left);
                    tracing::warn!(
                        batch_len,
                        "OpenRouter embedding batch was too large; splitting batch: {}",
                        msg
                    );
                }
                Err(err) => return Err(err),
            }
        }

        Ok(ordered_embeddings)
    }

    async fn request_batch(
        &self,
        texts: &[String],
        input_type: &str,
        estimated_tokens: usize,
        metrics: OpenRouterMetricsHandle,
    ) -> Result<Vec<Embedding>, OpenRouterBatchError> {
        let request = EmbeddingRequest {
            model: &self.model,
            input: texts,
            encoding_format: self.config.encoding_format.as_str(),
            dimensions: self.dim,
            input_type,
            provider: self.config.provider.as_ref(),
        };

        let mut last_retryable = None;
        for attempt in 0..=MAX_RETRIES {
            let request_index = metrics.lock().unwrap().start_request();
            let request_start = Instant::now();
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
                        let latency = request_start.elapsed();
                        metrics.lock().unwrap().record_request(
                            latency,
                            texts.len(),
                            estimated_tokens,
                            0,
                            self.dim,
                            true,
                        );
                        tracing::debug!(
                            openrouter_request_index = request_index,
                            openrouter_retry_attempt = attempt,
                            openrouter_input_count = texts.len(),
                            openrouter_estimated_tokens = estimated_tokens,
                            openrouter_latency_secs = latency.as_secs_f64(),
                            http_status = status.as_u16(),
                            "OpenRouter embedding response body read failed"
                        );
                        OpenRouterBatchError::Fatal(format!(
                            "OpenRouter response body read failed after {status}: {e}"
                        ))
                    })?;
                    let latency = request_start.elapsed();
                    let embeddings = match parse_embeddings_response(&body, self.dim, texts.len()) {
                        Ok(embeddings) => embeddings,
                        Err(err) => {
                            metrics.lock().unwrap().record_request(
                                latency,
                                texts.len(),
                                estimated_tokens,
                                0,
                                self.dim,
                                true,
                            );
                            tracing::debug!(
                                openrouter_request_index = request_index,
                                openrouter_retry_attempt = attempt,
                                openrouter_input_count = texts.len(),
                                openrouter_estimated_tokens = estimated_tokens,
                                openrouter_latency_secs = latency.as_secs_f64(),
                                http_status = status.as_u16(),
                                "OpenRouter embedding response parse failed"
                            );
                            return Err(OpenRouterBatchError::Fatal(err));
                        }
                    };
                    metrics.lock().unwrap().record_request(
                        latency,
                        texts.len(),
                        estimated_tokens,
                        embeddings.len(),
                        self.dim,
                        false,
                    );
                    tracing::debug!(
                        openrouter_request_index = request_index,
                        openrouter_retry_attempt = attempt,
                        openrouter_input_count = texts.len(),
                        openrouter_estimated_tokens = estimated_tokens,
                        openrouter_latency_secs = latency.as_secs_f64(),
                        http_status = status.as_u16(),
                        openrouter_response_vectors = embeddings.len(),
                        openrouter_response_dim = self.dim,
                        "OpenRouter embedding request completed"
                    );
                    return Ok(embeddings);
                }
                Ok(response) => {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    let latency = request_start.elapsed();
                    metrics.lock().unwrap().record_request(
                        latency,
                        texts.len(),
                        estimated_tokens,
                        0,
                        self.dim,
                        true,
                    );
                    tracing::debug!(
                        openrouter_request_index = request_index,
                        openrouter_retry_attempt = attempt,
                        openrouter_input_count = texts.len(),
                        openrouter_estimated_tokens = estimated_tokens,
                        openrouter_latency_secs = latency.as_secs_f64(),
                        http_status = status.as_u16(),
                        "OpenRouter embedding request failed"
                    );
                    let msg = format!(
                        "OpenRouter embeddings request failed with HTTP {status}: {}",
                        body_snippet(&body)
                    );
                    if is_payload_too_large(status, &body) {
                        return Err(OpenRouterBatchError::PayloadTooLarge(msg));
                    }
                    if is_retryable_status(status) && attempt < MAX_RETRIES {
                        metrics.lock().unwrap().record_retry();
                        last_retryable = Some(msg);
                        sleep_for_retry(attempt).await;
                        continue;
                    }
                    return Err(OpenRouterBatchError::Fatal(msg));
                }
                Err(err) if is_retryable_reqwest_error(&err) && attempt < MAX_RETRIES => {
                    let latency = request_start.elapsed();
                    metrics.lock().unwrap().record_request(
                        latency,
                        texts.len(),
                        estimated_tokens,
                        0,
                        self.dim,
                        true,
                    );
                    tracing::debug!(
                        openrouter_request_index = request_index,
                        openrouter_retry_attempt = attempt,
                        openrouter_input_count = texts.len(),
                        openrouter_estimated_tokens = estimated_tokens,
                        openrouter_latency_secs = latency.as_secs_f64(),
                        "OpenRouter embedding request transport failed"
                    );
                    metrics.lock().unwrap().record_retry();
                    last_retryable = Some(format!("OpenRouter request failed: {err}"));
                    sleep_for_retry(attempt).await;
                    continue;
                }
                Err(err) => {
                    let latency = request_start.elapsed();
                    metrics.lock().unwrap().record_request(
                        latency,
                        texts.len(),
                        estimated_tokens,
                        0,
                        self.dim,
                        true,
                    );
                    tracing::debug!(
                        openrouter_request_index = request_index,
                        openrouter_retry_attempt = attempt,
                        openrouter_input_count = texts.len(),
                        openrouter_estimated_tokens = estimated_tokens,
                        openrouter_latency_secs = latency.as_secs_f64(),
                        "OpenRouter embedding request transport failed"
                    );
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
