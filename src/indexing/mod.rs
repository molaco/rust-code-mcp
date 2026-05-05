//! Legacy indexing facade.

pub mod tantivy_adapter;

pub use rust_code_mcp_indexing::{
    get_snapshot_path, BackupManager, ConsistencyChecker, ConsistencyReport, ErrorCategory,
    ErrorCollector, ErrorDetail, IndexFileResult, IndexStats, IndexerConfig, IndexerCore,
    IndexerCoreConfig, IndexingError, IncrementalIndexer, ProcessedFile, UnifiedIndexer,
    retry_sync_with_backoff, retry_with_backoff,
};
pub use tantivy_adapter::TantivyAdapter;

pub mod backup {
    pub use rust_code_mcp_indexing::backup::*;
}

pub mod consistency {
    pub use rust_code_mcp_indexing::consistency::*;
}

pub mod error {
    pub use rust_code_mcp_indexing::error::*;
}

pub mod errors {
    pub use rust_code_mcp_indexing::errors::*;
}

pub mod incremental {
    pub use rust_code_mcp_indexing::incremental::*;
}

pub mod indexer_core {
    pub use rust_code_mcp_indexing::indexer_core::*;
}

pub mod merkle {
    pub use rust_code_mcp_indexing::merkle::*;
}

pub mod retry {
    pub use rust_code_mcp_indexing::retry::*;
}

pub mod unified {
    pub use rust_code_mcp_indexing::unified::*;
}
