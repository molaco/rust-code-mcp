//! Request DTOs sent to the OpenRouter embeddings endpoint.

use crate::embeddings::openrouter::config::OpenRouterProviderPreferences;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub(super) struct EmbeddingRequest<'a> {
    pub(super) model: &'a str,
    pub(super) input: &'a [String],
    pub(super) encoding_format: &'a str,
    pub(super) dimensions: usize,
    pub(super) input_type: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) provider: Option<&'a OpenRouterProviderPreferences>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::openrouter::config::OpenRouterProviderSort;

    #[test]
    fn serializes_provider_preferences_only_when_configured() {
        let input = vec!["example".to_string()];
        let provider = OpenRouterProviderPreferences {
            sort: Some(OpenRouterProviderSort::Throughput),
            preferred_min_throughput: Some(5000),
            preferred_max_latency: Some(2.0),
        };
        let request = EmbeddingRequest {
            model: "qwen/qwen3-embedding-8b",
            input: &input,
            encoding_format: "float",
            dimensions: 4096,
            input_type: "search_document",
            provider: Some(&provider),
        };

        let value = serde_json::to_value(&request).unwrap();

        assert_eq!(value["provider"]["sort"], "throughput");
        assert_eq!(value["provider"]["preferred_min_throughput"], 5000);
        assert_eq!(value["provider"]["preferred_max_latency"], 2.0);

        let request_without_provider = EmbeddingRequest {
            provider: None,
            ..request
        };
        let value = serde_json::to_value(&request_without_provider).unwrap();

        assert!(value.get("provider").is_none());
    }
}
