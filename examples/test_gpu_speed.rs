//! Qwen3 smoke test — verify the embedder constructs, runs on GPU,
//! produces non-zero vectors of the right dim, and that
//! embed_documents vs embed_queries produce DIFFERENT vectors for
//! the same input (proves the instruction prefix is applied).

use file_search_mcp::embeddings::{EmbeddingBackend, EmbeddingGenerator};
use std::time::Instant;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with_writer(std::io::stderr)
        .init();

    println!("{}", "=".repeat(60));
    println!("Qwen3 SMOKE TEST");
    println!("{}", "=".repeat(60));

    let backend = EmbeddingBackend::default();
    println!("Backend identity: {}", backend.identity());
    println!("Expected dim:     {}", backend.dim());

    let start = Instant::now();
    let generator = EmbeddingGenerator::new()?;
    println!("Generator init:   {:.2}s", start.elapsed().as_secs_f32());
    println!("Reported dim:     {}", generator.dimensions());
    assert_eq!(generator.dimensions(), backend.dim(), "dim mismatch");

    // Basic document embedding.
    let docs = vec!["fn add(a: i32, b: i32) -> i32 { a + b }".to_string()];
    let t = Instant::now();
    let doc_vecs = generator.embed_documents(docs.clone()).await?;
    println!("\nembed_documents 1 chunk: {:.3}s", t.elapsed().as_secs_f32());
    assert_eq!(doc_vecs.len(), 1);
    assert_eq!(doc_vecs[0].len(), backend.dim(), "doc dim wrong");
    assert!(doc_vecs[0].iter().any(|&x| x != 0.0), "doc vec is all zeros");
    println!("  vec[0..4] = {:?}", &doc_vecs[0][..4]);

    // Query embedding for the SAME input — must differ (instruction prefix).
    let t = Instant::now();
    let query_vecs = generator.embed_queries(docs.clone()).await?;
    println!("\nembed_queries 1 chunk: {:.3}s", t.elapsed().as_secs_f32());
    assert_eq!(query_vecs.len(), 1);
    assert_eq!(query_vecs[0].len(), backend.dim(), "query dim wrong");
    println!("  vec[0..4] = {:?}", &query_vecs[0][..4]);

    let diff: f32 = doc_vecs[0]
        .iter()
        .zip(query_vecs[0].iter())
        .map(|(a, b)| (a - b).abs())
        .sum();
    println!("\nL1 distance(doc, query) for same input: {:.4}", diff);
    assert!(
        diff > 0.01,
        "doc and query vectors are too similar — instruction prefix may not be applied"
    );

    // Batch throughput. Use a modest batch to avoid OOM on first run.
    let batch: Vec<String> = (0..32)
        .map(|i| format!("fn f{i}(x: i32) -> i32 {{ x + {i} }}"))
        .collect();
    let t = Instant::now();
    let batch_vecs = generator.embed_documents(batch.clone()).await?;
    let elapsed = t.elapsed().as_secs_f32();
    println!(
        "\nembed_documents 32 chunks: {:.3}s ({:.1} chunks/sec)",
        elapsed,
        32.0 / elapsed
    );
    assert_eq!(batch_vecs.len(), 32);

    println!("\n{}", "=".repeat(60));
    println!("SMOKE TEST PASSED");
    println!("{}", "=".repeat(60));
    Ok(())
}
