//! Legacy vector store facade.

pub use rust_code_mcp_vector_store::{
    LanceDbBackend, SearchResult, VectorStore, VectorStoreBackend, VectorStoreConfig,
    VectorStoreError,
};

pub mod error {
    pub use rust_code_mcp_vector_store::error::*;
}

pub mod lancedb {
    pub use rust_code_mcp_vector_store::lancedb::*;
}

pub mod traits {
    pub use rust_code_mcp_vector_store::traits::*;
}
