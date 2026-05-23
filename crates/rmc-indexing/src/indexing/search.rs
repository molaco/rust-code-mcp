//! Search-facing indexing facade.

use std::path::Path;

use anyhow::Result;
use rmc_config::config::indexer::TantivyConfig;
use rmc_engine::search::Bm25Search;

use crate::indexing::tantivy_adapter::TantivyAdapter;

/// Open BM25 search for an existing Tantivy index.
pub fn open_bm25_search(tantivy_path: &Path) -> Result<Bm25Search> {
    let config = TantivyConfig::default(tantivy_path);
    TantivyAdapter::new(config).and_then(|adapter| adapter.create_bm25_search())
}
