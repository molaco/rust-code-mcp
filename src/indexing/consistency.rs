//! Consistency checker for validating and repairing index integrity
//!
//! Verifies that Tantivy and vector store indexes are in sync

use crate::chunker::ChunkId;
use anyhow::{Context, Result};
use std::collections::HashSet;
use tantivy::{Index, IndexReader};
use tantivy::schema::Value;
use crate::vector_store::VectorStore;
use crate::schema::ChunkSchema;

/// Results from a consistency check
#[derive(Debug, Clone)]
pub struct ConsistencyReport {
    /// Number of chunks in Tantivy
    pub tantivy_count: usize,
    /// Number of chunks in vector store
    pub vector_count: usize,
    /// Chunk IDs present in Tantivy but missing from vector store
    pub missing_from_vectors: Vec<ChunkId>,
    /// Chunk IDs present in vector store but missing from Tantivy
    pub missing_from_tantivy: Vec<ChunkId>,
    /// Whether the indexes are consistent
    pub is_consistent: bool,
}

impl ConsistencyReport {
    /// Print a human-readable summary
    pub fn print_summary(&self) {
        println!("\n=== Index Consistency Report ===");
        println!("Tantivy chunks: {}", self.tantivy_count);
        println!("Vector chunks:  {}", self.vector_count);

        if self.is_consistent {
            println!("✓ Indexes are CONSISTENT");
        } else {
            println!("✗ Indexes are INCONSISTENT");

            if !self.missing_from_vectors.is_empty() {
                println!("\nMissing from vector store: {} chunks", self.missing_from_vectors.len());
                if self.missing_from_vectors.len() <= 10 {
                    for chunk_id in &self.missing_from_vectors {
                        println!("  - {:?}", chunk_id);
                    }
                } else {
                    println!("  (showing first 10)");
                    for chunk_id in self.missing_from_vectors.iter().take(10) {
                        println!("  - {:?}", chunk_id);
                    }
                }
            }

            if !self.missing_from_tantivy.is_empty() {
                println!("\nMissing from Tantivy: {} chunks", self.missing_from_tantivy.len());
                if self.missing_from_tantivy.len() <= 10 {
                    for chunk_id in &self.missing_from_tantivy {
                        println!("  - {:?}", chunk_id);
                    }
                } else {
                    println!("  (showing first 10)");
                    for chunk_id in self.missing_from_tantivy.iter().take(10) {
                        println!("  - {:?}", chunk_id);
                    }
                }
            }
        }
        println!("================================\n");
    }
}

/// Consistency checker for index integrity
pub struct ConsistencyChecker {
    tantivy_index: Index,
    vector_store: VectorStore,
    schema: ChunkSchema,
}

impl ConsistencyChecker {
    /// Create a new consistency checker
    pub fn new(
        tantivy_index: Index,
        vector_store: VectorStore,
        schema: ChunkSchema,
    ) -> Self {
        Self {
            tantivy_index,
            vector_store,
            schema,
        }
    }

    /// Check consistency between Tantivy and vector store indexes
    pub async fn check(&self) -> Result<ConsistencyReport> {
        tracing::info!("Starting consistency check...");

        // Get all chunk IDs from Tantivy
        let tantivy_ids = self.get_tantivy_chunk_ids()?;
        tracing::info!("Found {} chunks in Tantivy", tantivy_ids.len());

        // Get count from vector store
        let vector_count = self.vector_store.count().await
            .map_err(|e| anyhow::anyhow!("Failed to count vector store chunks: {}", e))?;
        tracing::info!("Found {} chunks in vector store", vector_count);

        // For now, we can only check counts
        // TODO: Implement full chunk ID verification
        //       (requires adding a method to list all chunk IDs)
        let is_consistent = tantivy_ids.len() == vector_count;

        let report = ConsistencyReport {
            tantivy_count: tantivy_ids.len(),
            vector_count,
            missing_from_vectors: Vec::new(), // Would require vector store ID listing
            missing_from_tantivy: Vec::new(),
            is_consistent,
        };

        tracing::info!("Consistency check complete: {}", if is_consistent { "OK" } else { "FAILED" });

        Ok(report)
    }

    /// Get all chunk IDs from Tantivy index
    fn get_tantivy_chunk_ids(&self) -> Result<HashSet<ChunkId>> {
        let reader: IndexReader = self.tantivy_index
            .reader_builder()
            .reload_policy(tantivy::ReloadPolicy::Manual)
            .try_into()
            .context("Failed to create Tantivy reader")?;

        let searcher = reader.searcher();
        let mut chunk_ids = HashSet::new();

        // Iterate through all documents
        for segment_reader in searcher.segment_readers() {
            let store_reader = segment_reader
                .get_store_reader(0)
                .context("Failed to get store reader")?;

            for doc_id in 0..segment_reader.max_doc() {
                if let Ok(doc) = store_reader.get::<tantivy::TantivyDocument>(doc_id) {
                    // Extract chunk_id field
                    if let Some(chunk_id_field) = doc.get_first(self.schema.chunk_id) {
                        if let Some(chunk_id_str) = chunk_id_field.as_str() {
                            if let Ok(chunk_id) = ChunkId::from_string(chunk_id_str) {
                                chunk_ids.insert(chunk_id);
                            }
                        }
                    }
                }
            }
        }

        Ok(chunk_ids)
    }

    /// Repair inconsistencies by reindexing missing chunks
    ///
    /// This is a placeholder for future implementation
    pub async fn repair(&self, _report: &ConsistencyReport) -> Result<()> {
        // TODO: Implement repair logic
        // - For chunks missing from vector store: re-embed and re-index
        // - For chunks missing from Tantivy: re-index from source or remove from vector store
        anyhow::bail!("Repair not yet implemented. Use force reindex instead.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_consistency_report_creation() {
        let report = ConsistencyReport {
            tantivy_count: 100,
            vector_count: 100,
            missing_from_vectors: Vec::new(),
            missing_from_tantivy: Vec::new(),
            is_consistent: true,
        };

        assert!(report.is_consistent);
        assert_eq!(report.tantivy_count, 100);
        assert_eq!(report.vector_count, 100);
    }

    #[test]
    fn test_inconsistent_report() {
        let report = ConsistencyReport {
            tantivy_count: 100,
            vector_count: 95,
            missing_from_vectors: vec![ChunkId::new()],
            missing_from_tantivy: Vec::new(),
            is_consistent: false,
        };

        assert!(!report.is_consistent);
        assert_eq!(report.missing_from_vectors.len(), 1);
    }
}
