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
