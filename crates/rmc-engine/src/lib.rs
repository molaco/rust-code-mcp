//! rmc-engine — foundation crate for parsing, chunking, embeddings, vector storage, search.

pub mod chunker;
#[cfg(feature = "embeddings")]
pub mod embeddings;
pub mod parser;
pub mod schema;
pub mod search;
#[cfg(feature = "vector-store")]
pub mod vector_store;
