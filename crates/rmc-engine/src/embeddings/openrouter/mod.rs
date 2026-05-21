//! OpenRouter embeddings backend.

mod batch;
mod client;
mod config;
mod metrics;
mod request;
mod response;
mod retry;

pub use config::{
    openrouter_runtime_config, OpenRouterEncodingFormat, OpenRouterProviderPreferences,
    OpenRouterProviderSort, OpenRouterRuntimeConfig,
};
pub(in crate::embeddings) use client::OpenRouterEmbedder;
