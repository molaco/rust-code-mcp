//! Search-facing indexing facade.

use std::path::Path;

use anyhow::{Context, Result};
use rmc_engine::search::Bm25Search;

/// Open BM25 search for an existing Tantivy index.
pub fn open_bm25_search(tantivy_path: &Path) -> Result<Bm25Search> {
    if !tantivy_path.join("meta.json").is_file() {
        anyhow::bail!(
            "Tantivy index does not exist at {}",
            tantivy_path.display()
        );
    }

    let index = tantivy::Index::open_in_dir(tantivy_path)
        .with_context(|| format!("Failed to open Tantivy index at {}", tantivy_path.display()))?;

    Bm25Search::from_index(index)
        .map_err(|error| anyhow::anyhow!("Failed to create Bm25Search: {}", error))
}

#[cfg(test)]
mod tests {
    use super::*;

    use rmc_engine::schema::ChunkSchema;
    use tempfile::TempDir;

    #[test]
    fn open_bm25_search_returns_err_for_missing_path() {
        let temp_dir = TempDir::new().unwrap();
        let missing_path = temp_dir.path().join("missing");

        let search = open_bm25_search(&missing_path);

        assert!(search.is_err());
    }

    #[test]
    fn open_bm25_search_does_not_create_missing_path() {
        let temp_dir = TempDir::new().unwrap();
        let missing_path = temp_dir.path().join("missing");

        let _ = open_bm25_search(&missing_path);

        assert!(!missing_path.exists());
    }

    #[test]
    fn open_bm25_search_returns_err_for_directory_without_meta_json() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().join("tantivy");
        std::fs::create_dir(&index_path).unwrap();
        let entry_count_before = std::fs::read_dir(&index_path).unwrap().count();

        let search = open_bm25_search(&index_path);

        let entry_count_after = std::fs::read_dir(&index_path).unwrap().count();
        assert!(search.is_err());
        assert_eq!(entry_count_after, entry_count_before);
        assert!(!index_path.join("meta.json").exists());
    }

    #[test]
    fn open_bm25_search_opens_valid_tantivy_index() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().join("tantivy");
        std::fs::create_dir(&index_path).unwrap();
        let schema = ChunkSchema::new();
        tantivy::Index::create_in_dir(&index_path, schema.schema()).unwrap();

        let search = open_bm25_search(&index_path);

        assert!(search.is_ok(), "Failed to open BM25 search: {:?}", search.err());
    }
}
