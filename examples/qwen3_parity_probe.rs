use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::PathBuf;

use rust_code_mcp::embeddings::EmbeddingGenerator;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct EmbeddingSnapshot {
    documents: Vec<Vec<f32>>,
    queries: Vec<Vec<f32>>,
}

#[derive(Debug, Default)]
struct DeltaSummary {
    vector_count: usize,
    value_count: usize,
    min_cosine: f64,
    mean_cosine: f64,
    max_abs_delta: f64,
    mean_abs_delta: f64,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 || !matches!(args[1].as_str(), "--write" | "--compare") {
        eprintln!("usage: qwen3_parity_probe --write <path> | --compare <path>");
        std::process::exit(2);
    }

    let path = PathBuf::from(&args[2]);
    let snapshot = generate_snapshot().await?;

    match args[1].as_str() {
        "--write" => {
            let file = File::create(&path)?;
            serde_json::to_writer(BufWriter::new(file), &snapshot)?;
            println!("wrote {}", path.display());
        }
        "--compare" => {
            let file = File::open(&path)?;
            let baseline: EmbeddingSnapshot = serde_json::from_reader(BufReader::new(file))?;
            let doc_summary = compare_vectors(&baseline.documents, &snapshot.documents)?;
            let query_summary = compare_vectors(&baseline.queries, &snapshot.queries)?;
            print_summary("documents", &doc_summary);
            print_summary("queries", &query_summary);
        }
        _ => unreachable!(),
    }

    Ok(())
}

async fn generate_snapshot() -> Result<EmbeddingSnapshot, Box<dyn std::error::Error>> {
    let generator = EmbeddingGenerator::new()?;
    let documents = generator.embed_documents(document_corpus()).await?;
    let queries = generator.embed_queries(query_corpus()).await?;
    Ok(EmbeddingSnapshot { documents, queries })
}

fn document_corpus() -> Vec<String> {
    let mut docs = Vec::new();

    for idx in 0..10 {
        docs.push(format!("fn short_{idx}() -> usize {{ {idx} }}"));
    }

    for idx in 0..10 {
        docs.push(format!(
            "pub async fn load_{idx}(path: &Path) -> anyhow::Result<Vec<String>> {{\n    let content = tokio::fs::read_to_string(path).await?;\n    Ok(content.lines().map(str::to_owned).collect())\n}}"
        ));
    }

    for idx in 0..10 {
        docs.push(format!(
            "impl Worker{idx} {{\n    pub fn run(&mut self, jobs: &[Job]) -> Result<Report, Error> {{\n        let mut report = Report::default();\n        for job in jobs {{\n            if job.is_ready() {{\n                let output = self.execute(job)?;\n                report.record(job.id(), output.status());\n            }} else {{\n                report.defer(job.id());\n            }}\n        }}\n        report.finish()?;\n        Ok(report)\n    }}\n}}"
        ));
    }

    docs
}

fn query_corpus() -> Vec<String> {
    vec![
        "find async file loading code".to_string(),
        "where are job reports finalized".to_string(),
        "small constant return helper".to_string(),
        "error propagation with anyhow result".to_string(),
        "worker execution loop over ready jobs".to_string(),
    ]
}

fn compare_vectors(
    baseline: &[Vec<f32>],
    candidate: &[Vec<f32>],
) -> Result<DeltaSummary, Box<dyn std::error::Error>> {
    if baseline.len() != candidate.len() {
        return Err(format!(
            "vector count mismatch: baseline={} candidate={}",
            baseline.len(),
            candidate.len()
        )
        .into());
    }

    let mut summary = DeltaSummary {
        min_cosine: 1.0,
        ..DeltaSummary::default()
    };
    let mut cosine_total = 0.0;
    let mut abs_delta_total = 0.0;

    for (left, right) in baseline.iter().zip(candidate) {
        if left.len() != right.len() {
            return Err(format!(
                "dimension mismatch: baseline={} candidate={}",
                left.len(),
                right.len()
            )
            .into());
        }

        let cosine = cosine(left, right);
        summary.min_cosine = summary.min_cosine.min(cosine);
        cosine_total += cosine;
        summary.vector_count += 1;

        for (&a, &b) in left.iter().zip(right) {
            let delta = (a as f64 - b as f64).abs();
            summary.max_abs_delta = summary.max_abs_delta.max(delta);
            abs_delta_total += delta;
            summary.value_count += 1;
        }
    }

    if summary.vector_count > 0 {
        summary.mean_cosine = cosine_total / summary.vector_count as f64;
    }
    if summary.value_count > 0 {
        summary.mean_abs_delta = abs_delta_total / summary.value_count as f64;
    }

    Ok(summary)
}

fn cosine(left: &[f32], right: &[f32]) -> f64 {
    let mut dot = 0.0;
    let mut left_norm = 0.0;
    let mut right_norm = 0.0;

    for (&a, &b) in left.iter().zip(right) {
        let a = a as f64;
        let b = b as f64;
        dot += a * b;
        left_norm += a * a;
        right_norm += b * b;
    }

    dot / (left_norm.sqrt() * right_norm.sqrt())
}

fn print_summary(label: &str, summary: &DeltaSummary) {
    println!(
        "{label}: vectors={} values={} min_cosine={:.9} mean_cosine={:.9} max_abs_delta={:.9} mean_abs_delta={:.9}",
        summary.vector_count,
        summary.value_count,
        summary.min_cosine,
        summary.mean_cosine,
        summary.max_abs_delta,
        summary.mean_abs_delta
    );
}
