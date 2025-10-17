//! Embedding generation using fastembed
//!
//! Generates embeddings for code chunks using local ONNX models

use crate::chunker::{ChunkId, CodeChunk};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

/// An embedding vector (384 dimensions for all-MiniLM-L6-v2)
pub type Embedding = Vec<f32>;

/// A chunk with its generated embedding
#[derive(Debug, Clone)]
pub struct ChunkWithEmbedding {
    pub chunk_id: ChunkId,
    pub embedding: Embedding,
}

/// Embedding generator using fastembed
pub struct EmbeddingGenerator {
    model: TextEmbedding,
    dimensions: usize,
}

impl EmbeddingGenerator {
    /// Create a new embedding generator with the default model (all-MiniLM-L6-v2)
    ///
    /// This model:
    /// - 384 dimensions
    /// - ~80MB download
    /// - Good balance of speed and quality
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::AllMiniLML6V2)
                .with_show_download_progress(true),
        )?;

        Ok(Self {
            model,
            dimensions: 384,
        })
    }

    /// Get the embedding dimensions
    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    /// Generate embedding for a single text
    pub fn embed(&self, text: &str) -> Result<Embedding, Box<dyn std::error::Error>> {
        let embeddings = self.model.embed(vec![text], None)?;
        embeddings
            .into_iter()
            .next()
            .ok_or_else(|| "No embedding generated".into())
    }

    /// Generate embeddings for multiple texts (batch processing)
    pub fn embed_batch(
        &self,
        texts: Vec<String>,
    ) -> Result<Vec<Embedding>, Box<dyn std::error::Error>> {
        let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
        Ok(self.model.embed(text_refs, None)?)
    }

    /// Generate embeddings for code chunks
    pub fn embed_chunks(
        &self,
        chunks: &[CodeChunk],
    ) -> Result<Vec<ChunkWithEmbedding>, Box<dyn std::error::Error>> {
        // Format chunks for embedding
        let formatted: Vec<String> = chunks
            .iter()
            .map(|chunk| chunk.format_for_embedding())
            .collect();

        // Generate embeddings in batch
        let embeddings = self.embed_batch(formatted)?;

        // Pair with chunk IDs
        let results: Vec<ChunkWithEmbedding> = chunks
            .iter()
            .zip(embeddings.into_iter())
            .map(|(chunk, embedding)| ChunkWithEmbedding {
                chunk_id: chunk.id,
                embedding,
            })
            .collect();

        Ok(results)
    }
}

/// Embedding pipeline with batch processing and progress reporting
pub struct EmbeddingPipeline {
    generator: EmbeddingGenerator,
    batch_size: usize,
}

impl EmbeddingPipeline {
    /// Create a new embedding pipeline
    pub fn new(generator: EmbeddingGenerator) -> Self {
        Self {
            generator,
            batch_size: 32, // Process 32 chunks at a time
        }
    }

    /// Create with custom batch size
    pub fn with_batch_size(generator: EmbeddingGenerator, batch_size: usize) -> Self {
        Self {
            generator,
            batch_size,
        }
    }

    /// Process chunks with progress callback
    ///
    /// The progress callback receives (current, total) for each batch processed
    pub fn process_chunks<F>(
        &self,
        chunks: Vec<CodeChunk>,
        mut progress: F,
    ) -> Result<Vec<ChunkWithEmbedding>, Box<dyn std::error::Error>>
    where
        F: FnMut(usize, usize),
    {
        let total = chunks.len();
        let mut results = Vec::new();

        // Process in batches
        for (batch_idx, batch) in chunks.chunks(self.batch_size).enumerate() {
            let batch_results = self.generator.embed_chunks(batch)?;
            results.extend(batch_results);

            let processed = (batch_idx + 1) * self.batch_size;
            progress(processed.min(total), total);
        }

        Ok(results)
    }

    /// Get the embedding dimensions
    pub fn dimensions(&self) -> usize {
        self.generator.dimensions()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::{ChunkContext, CodeChunk};
    use std::path::PathBuf;

    fn create_test_chunk(content: &str, symbol_name: &str) -> CodeChunk {
        CodeChunk {
            id: ChunkId::new(),
            content: content.to_string(),
            context: ChunkContext {
                file_path: PathBuf::from("test.rs"),
                module_path: vec!["crate".to_string()],
                symbol_name: symbol_name.to_string(),
                symbol_kind: "function".to_string(),
                docstring: None,
                imports: vec![],
                outgoing_calls: vec![],
                line_start: 1,
                line_end: 10,
            },
            overlap_prev: None,
            overlap_next: None,
        }
    }

    #[test]
    #[ignore] // Requires model download, run with --ignored
    fn test_generator_creation() {
        let generator = EmbeddingGenerator::new();
        assert!(generator.is_ok());

        let generator = generator.unwrap();
        assert_eq!(generator.dimensions(), 384);
    }

    #[test]
    #[ignore] // Requires model download
    fn test_embed_single() {
        let generator = EmbeddingGenerator::new().unwrap();
        let embedding = generator.embed("fn test() {}").unwrap();

        assert_eq!(embedding.len(), 384);
        // Check that it's not all zeros
        assert!(embedding.iter().any(|&x| x != 0.0));
    }

    #[test]
    #[ignore] // Requires model download
    fn test_embed_batch() {
        let generator = EmbeddingGenerator::new().unwrap();
        let texts = vec![
            "fn test1() {}".to_string(),
            "fn test2() {}".to_string(),
            "struct Data {}".to_string(),
        ];

        let embeddings = generator.embed_batch(texts).unwrap();

        assert_eq!(embeddings.len(), 3);
        for embedding in &embeddings {
            assert_eq!(embedding.len(), 384);
        }
    }

    #[test]
    #[ignore] // Requires model download
    fn test_embed_chunks() {
        let generator = EmbeddingGenerator::new().unwrap();

        let chunks = vec![
            create_test_chunk("fn test1() {}", "test1"),
            create_test_chunk("fn test2() {}", "test2"),
        ];

        let results = generator.embed_chunks(&chunks).unwrap();

        assert_eq!(results.len(), 2);
        for result in &results {
            assert_eq!(result.embedding.len(), 384);
        }
    }

    #[test]
    #[ignore] // Requires model download
    fn test_pipeline() {
        let generator = EmbeddingGenerator::new().unwrap();
        let pipeline = EmbeddingPipeline::with_batch_size(generator, 2);

        let chunks = vec![
            create_test_chunk("fn test1() {}", "test1"),
            create_test_chunk("fn test2() {}", "test2"),
            create_test_chunk("fn test3() {}", "test3"),
        ];

        let mut progress_calls = 0;
        let results = pipeline
            .process_chunks(chunks, |current, total| {
                progress_calls += 1;
                println!("Progress: {}/{}", current, total);
            })
            .unwrap();

        assert_eq!(results.len(), 3);
        assert!(progress_calls > 0, "Progress callback should be called");
    }

    #[test]
    #[ignore] // Requires model download
    fn test_embedding_similarity() {
        let generator = EmbeddingGenerator::new().unwrap();

        // Similar functions should have similar embeddings
        let emb1 = generator.embed("fn add(a: i32, b: i32) -> i32 { a + b }").unwrap();
        let emb2 = generator.embed("fn sum(x: i32, y: i32) -> i32 { x + y }").unwrap();
        let emb3 = generator.embed("struct Point { x: f64, y: f64 }").unwrap();

        // Cosine similarity
        let sim_12 = cosine_similarity(&emb1, &emb2);
        let sim_13 = cosine_similarity(&emb1, &emb3);

        // Similar code should be more similar than dissimilar code
        assert!(
            sim_12 > sim_13,
            "Similar functions should be more similar than unrelated code"
        );
    }

    // Helper: compute cosine similarity
    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        dot / (norm_a * norm_b)
    }
}
