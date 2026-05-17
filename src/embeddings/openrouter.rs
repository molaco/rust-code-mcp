//! OpenRouter embeddings backend.

use crate::embeddings::backend::{EmbeddingBackend, EmbeddingRuntime};
use crate::embeddings::{Embedding, EmbeddingError, EmbeddingTokenCounter};
use futures::stream::{self, StreamExt};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

pub(super) struct OpenRouterEmbedder {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
    dim: usize,
    config: OpenRouterRuntimeConfig,
    token_counter: Option<EmbeddingTokenCounter>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenRouterRuntimeConfig {
    pub max_batch_inputs: usize,
    pub max_batch_tokens: usize,
    pub concurrency: usize,
    pub encoding_format: OpenRouterEncodingFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenRouterEncodingFormat {
    Float,
}

impl OpenRouterEncodingFormat {
    pub fn as_str(self) -> &'static str {
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenRouterInput {
    original_index: usize,
    text: String,
    token_len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OpenRouterBatchPlan {
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, Default)]
struct OpenRouterRequestMetrics {
    request_count: usize,
    retry_count: usize,
    split_count: usize,
    failed_request_count: usize,
    total_latency: Duration,
    min_latency: Option<Duration>,
    max_latency: Duration,
    total_request_inputs: usize,
    max_request_inputs: usize,
    total_estimated_tokens: usize,
    max_estimated_tokens: usize,
    response_vector_count: usize,
    response_dim: Option<usize>,
}

type OpenRouterMetricsHandle = Arc<Mutex<OpenRouterRequestMetrics>>;

impl OpenRouterRequestMetrics {
    fn start_request(&mut self) -> usize {
        self.request_count += 1;
        self.request_count
    }

    fn record_request(
        &mut self,
        latency: Duration,
        input_count: usize,
        estimated_tokens: usize,
        response_vectors: usize,
        response_dim: usize,
        failed: bool,
    ) {
        self.total_latency += latency;
        self.min_latency = Some(
            self.min_latency
                .map(|min_latency| min_latency.min(latency))
                .unwrap_or(latency),
        );
        self.max_latency = self.max_latency.max(latency);
        self.total_request_inputs += input_count;
        self.max_request_inputs = self.max_request_inputs.max(input_count);
        self.total_estimated_tokens += estimated_tokens;
        self.max_estimated_tokens = self.max_estimated_tokens.max(estimated_tokens);
        self.response_vector_count += response_vectors;
        if response_vectors > 0 {
            self.response_dim = Some(response_dim);
        }
        if failed {
            self.failed_request_count += 1;
        }
    }

    fn record_retry(&mut self) {
        self.retry_count += 1;
    }

    fn record_split(&mut self) {
        self.split_count += 1;
    }

    fn avg_latency(&self) -> Duration {
        if self.request_count == 0 {
            Duration::ZERO
        } else {
            self.total_latency / self.request_count as u32
        }
    }
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
            "OpenRouter embedding runtime configured"
        );

        Ok(Self {
            client,
            api_key,
            base_url,
            model,
            dim: backend.dim(),
            config,
            token_counter,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenRouterInputBatch {
    inputs: Vec<OpenRouterInput>,
}

impl OpenRouterInputBatch {
    fn texts(&self) -> Vec<String> {
        self.inputs
            .iter()
            .map(|input| input.text.clone())
            .collect()
    }

    fn len(&self) -> usize {
        self.inputs.len()
    }

    fn max_token_len(&self) -> usize {
        self.inputs
            .iter()
            .map(|input| input.token_len)
            .max()
            .unwrap_or(0)
    }

    fn padded_tokens(&self) -> usize {
        self.len() * self.max_token_len()
    }

    fn split_at(self, mid: usize) -> (Self, Self) {
        let right = self.inputs[mid..].to_vec();
        let left = self.inputs[..mid].to_vec();
        (Self { inputs: left }, Self { inputs: right })
    }
}

fn plan_remote_input_batches(
    texts: Vec<String>,
    token_lengths: Vec<usize>,
    config: OpenRouterRuntimeConfig,
) -> Vec<OpenRouterInputBatch> {
    assert_eq!(
        texts.len(),
        token_lengths.len(),
        "OpenRouter planner requires one token length per text"
    );

    let mut inputs: Vec<OpenRouterInput> = texts
        .into_iter()
        .zip(token_lengths)
        .enumerate()
        .map(|(original_index, (text, token_len))| OpenRouterInput {
            original_index,
            text,
            token_len: token_len.max(1),
        })
        .collect();

    sort_openrouter_inputs(&mut inputs);
    plan_openrouter_batches(&inputs, config)
        .into_iter()
        .map(|plan| OpenRouterInputBatch {
            inputs: inputs[plan.start..plan.end].to_vec(),
        })
        .collect()
}

fn sort_openrouter_inputs(inputs: &mut [OpenRouterInput]) {
    inputs.sort_by_key(|input| (input.token_len, input.original_index));
}

fn plan_openrouter_batches(
    inputs: &[OpenRouterInput],
    config: OpenRouterRuntimeConfig,
) -> Vec<OpenRouterBatchPlan> {
    if inputs.is_empty() {
        return Vec::new();
    }

    let max_batch_inputs = config.max_batch_inputs.max(1);
    let max_batch_tokens = config.max_batch_tokens.max(1);
    let mut plans = Vec::new();
    let mut start = 0usize;
    let mut batch_len = 0usize;
    let mut batch_max_tokens = 0usize;

    for (idx, input) in inputs.iter().enumerate() {
        let item_tokens = input.token_len.max(1);
        let next_len = batch_len + 1;
        let next_max_tokens = batch_max_tokens.max(item_tokens);
        let exceeds_count = next_len > max_batch_inputs;
        let exceeds_token_budget = next_len * next_max_tokens > max_batch_tokens;

        if batch_len > 0 && (exceeds_count || exceeds_token_budget) {
            plans.push(OpenRouterBatchPlan { start, end: idx });
            start = idx;
            batch_len = 0;
            batch_max_tokens = 0;
        }

        batch_len += 1;
        batch_max_tokens = batch_max_tokens.max(item_tokens);
    }

    if batch_len > 0 {
        plans.push(OpenRouterBatchPlan {
            start,
            end: inputs.len(),
        });
    }

    plans
}

fn fallback_token_estimate(text: &str) -> usize {
    text.len().div_ceil(4).max(1)
}

fn restore_original_embedding_order(
    expected_count: usize,
    embeddings: Vec<(usize, Embedding)>,
) -> Result<Vec<Embedding>, String> {
    let mut output: Vec<Option<Embedding>> = vec![None; expected_count];
    for (original_index, embedding) in embeddings {
        if original_index >= expected_count {
            return Err(format!(
                "OpenRouter embedding result had out-of-range original index {} for {} inputs",
                original_index, expected_count
            ));
        }
        output[original_index] = Some(embedding);
    }

    output
        .into_iter()
        .enumerate()
        .map(|(idx, maybe)| {
            maybe.ok_or_else(|| {
                format!("OpenRouter embedding result omitted original index {idx}")
            })
        })
        .collect()
}

fn log_openrouter_request_metrics(
    metrics: &OpenRouterRequestMetrics,
    elapsed: Duration,
    embedding_count: usize,
) {
    let min_latency = metrics.min_latency.unwrap_or(Duration::ZERO);
    let avg_latency = metrics.avg_latency();
    let padded_tokens_per_sec = if elapsed.is_zero() {
        0.0
    } else {
        metrics.total_estimated_tokens as f64 / elapsed.as_secs_f64()
    };

    tracing::info!(
        openrouter_request_count = metrics.request_count,
        openrouter_retry_count = metrics.retry_count,
        openrouter_split_count = metrics.split_count,
        openrouter_failed_request_count = metrics.failed_request_count,
        openrouter_total_request_latency_secs = metrics.total_latency.as_secs_f64(),
        openrouter_min_request_latency_secs = min_latency.as_secs_f64(),
        openrouter_avg_request_latency_secs = avg_latency.as_secs_f64(),
        openrouter_max_request_latency_secs = metrics.max_latency.as_secs_f64(),
        openrouter_total_request_inputs = metrics.total_request_inputs,
        openrouter_max_request_inputs = metrics.max_request_inputs,
        openrouter_total_estimated_tokens = metrics.total_estimated_tokens,
        openrouter_max_estimated_tokens = metrics.max_estimated_tokens,
        openrouter_response_vector_count = metrics.response_vector_count,
        openrouter_response_dim = metrics.response_dim.unwrap_or(0),
        openrouter_embedding_count = embedding_count,
        openrouter_elapsed_secs = elapsed.as_secs_f64(),
        openrouter_padded_tokens_per_sec = padded_tokens_per_sec,
        "OpenRouter embedding request metrics"
    );
}

fn api_key_from_env() -> Result<String, EmbeddingError> {
    resolve_api_key(|key| std::env::var(key))
}

pub fn openrouter_runtime_config() -> OpenRouterRuntimeConfig {
    openrouter_runtime_config_from_env()
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

    #[test]
    fn request_metrics_tracks_counts_and_latency() {
        let mut metrics = OpenRouterRequestMetrics::default();

        assert_eq!(metrics.start_request(), 1);
        metrics.record_request(Duration::from_millis(100), 4, 40, 4, 4096, false);
        metrics.record_retry();
        metrics.record_split();
        assert_eq!(metrics.start_request(), 2);
        metrics.record_request(Duration::from_millis(300), 2, 20, 0, 4096, true);

        assert_eq!(metrics.request_count, 2);
        assert_eq!(metrics.retry_count, 1);
        assert_eq!(metrics.split_count, 1);
        assert_eq!(metrics.failed_request_count, 1);
        assert_eq!(metrics.total_request_inputs, 6);
        assert_eq!(metrics.max_request_inputs, 4);
        assert_eq!(metrics.total_estimated_tokens, 60);
        assert_eq!(metrics.max_estimated_tokens, 40);
        assert_eq!(metrics.response_vector_count, 4);
        assert_eq!(metrics.response_dim, Some(4096));
        assert_eq!(metrics.min_latency, Some(Duration::from_millis(100)));
        assert_eq!(metrics.max_latency, Duration::from_millis(300));
        assert_eq!(metrics.avg_latency(), Duration::from_millis(200));
    }

    #[test]
    fn sorts_openrouter_inputs_by_token_length_then_original_index() {
        let mut inputs = vec![
            openrouter_input(2, "c", 4),
            openrouter_input(0, "a", 8),
            openrouter_input(1, "b", 4),
        ];

        sort_openrouter_inputs(&mut inputs);

        let original_indices: Vec<usize> =
            inputs.iter().map(|input| input.original_index).collect();
        assert_eq!(original_indices, vec![1, 2, 0]);
    }

    #[test]
    fn plans_openrouter_batches_by_input_count() {
        let batches = plan_remote_input_batches(
            vec!["a", "b", "c", "d", "e"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            vec![1, 1, 1, 1, 1],
            test_config(2, 100, 4),
        );

        let batch_lens: Vec<usize> = batches.iter().map(OpenRouterInputBatch::len).collect();
        assert_eq!(batch_lens, vec![2, 2, 1]);
    }

    #[test]
    fn plans_openrouter_batches_by_padded_token_budget() {
        let batches = plan_remote_input_batches(
            vec!["a", "b", "c", "d"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            vec![4, 4, 8, 8],
            test_config(4, 16, 4),
        );

        let batch_lens: Vec<usize> = batches.iter().map(OpenRouterInputBatch::len).collect();
        let padded_tokens: Vec<usize> =
            batches.iter().map(OpenRouterInputBatch::padded_tokens).collect();

        assert_eq!(batch_lens, vec![2, 2]);
        assert_eq!(padded_tokens, vec![8, 16]);
    }

    #[test]
    fn plans_openrouter_oversize_input_as_single_batch() {
        let batches = plan_remote_input_batches(
            vec!["oversize", "small"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            vec![32, 2],
            test_config(4, 16, 4),
        );

        assert!(batches.iter().any(|batch| {
            batch.len() == 1
                && batch.inputs[0].original_index == 0
                && batch.inputs[0].token_len == 32
        }));
    }

    #[test]
    fn openrouter_planner_keeps_original_indices_for_order_restoration() {
        let batches = plan_remote_input_batches(
            vec!["third", "first", "second"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            vec![9, 3, 6],
            test_config(8, 100, 4),
        );
        let pairs: Vec<(usize, Embedding)> = batches
            .into_iter()
            .flat_map(|batch| batch.inputs)
            .map(|input| (input.original_index, vec![input.original_index as f32]))
            .collect();

        let restored = restore_original_embedding_order(3, pairs).unwrap();

        assert_eq!(restored, vec![vec![0.0], vec![1.0], vec![2.0]]);
    }

    #[test]
    fn openrouter_planner_benchmark_shape_targets_fewer_requests() {
        let text_count = 2084;
        let batches = plan_remote_input_batches(
            (0..text_count)
                .map(|idx| format!("chunk {idx}"))
                .collect(),
            vec![300; text_count],
            test_config(128, 131_072, 4),
        );

        assert!(
            (8..=20).contains(&batches.len()),
            "expected 8-20 batches, got {}",
            batches.len()
        );
    }

    #[test]
    fn fallback_token_estimate_is_deterministic_and_nonzero() {
        assert_eq!(fallback_token_estimate(""), 1);
        assert_eq!(fallback_token_estimate("abcd"), 1);
        assert_eq!(fallback_token_estimate("abcde"), 2);
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

    fn openrouter_input(original_index: usize, text: &str, token_len: usize) -> OpenRouterInput {
        OpenRouterInput {
            original_index,
            text: text.to_string(),
            token_len,
        }
    }

    fn test_config(
        max_batch_inputs: usize,
        max_batch_tokens: usize,
        concurrency: usize,
    ) -> OpenRouterRuntimeConfig {
        OpenRouterRuntimeConfig {
            max_batch_inputs,
            max_batch_tokens,
            concurrency,
            encoding_format: OpenRouterEncodingFormat::Float,
        }
    }
}
