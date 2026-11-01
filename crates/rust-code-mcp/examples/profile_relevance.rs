//! Compare embedding profiles by SEARCH QUALITY, not by throughput.
//!
//! Every measurement in the GPU track so far answered "how fast", and the
//! question "does the small model actually find the right file" was never
//! asked. This runner indexes the same corpus once per profile, runs the same
//! hand-labelled query set against each index, and prints one table.
//!
//! Two columns per profile, on purpose:
//!   * `vector` — vector search alone. This is the axis that can tell profiles
//!     apart, because it is the only part of the pipeline the embedding model
//!     touches.
//!   * `hybrid` — vector fused with BM25 by RRF, i.e. what the user actually
//!     gets. BM25 is identical across profiles, so it compresses the spread;
//!     reading only this column would understate a real difference between
//!     models, and reading only `vector` would overstate its effect on the
//!     product.
//!
//! Usage:
//!   cargo run --release --features embeddings --example profile_relevance -- \
//!       --profile local-cpu-small --profile local-gpu-bge
//!
//! `local-gpu-bge` additionally needs the `migraphx` feature and a system ONNX
//! Runtime built with MIGraphX; `openrouter-*` profiles need
//! RUST_CODE_MCP_OPENROUTER_API_KEY and cost money per run.

use anyhow::{Context, Result, bail};
use rmc_engine::embeddings::{EmbeddingBackend, resolve_profile};
use rmc_engine::search::{
    HybridSearch, RelevanceCase, RelevanceSummary, RepoRelPath, score_case,
};
use rmc_indexing::indexing::UnifiedIndexer;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// How many results are pulled per query. Must cover the deepest cutoff the
/// metric reports (recall@10), otherwise the cutoff would silently measure the
/// fetch limit instead of the ranking.
const RESULT_LIMIT: usize = 10;

#[derive(Debug, Deserialize)]
struct RelevanceDataset {
    cases: Vec<RelevanceCase>,
}

/// One profile's scores on both retrieval modes.
struct ProfileOutcome {
    profile: String,
    dim: usize,
    indexed_files: usize,
    total_chunks: usize,
    index_secs: f64,
    vector: RelevanceSummary,
    hybrid: RelevanceSummary,
    /// Labels naming a file that is not in the index at all.
    unreachable_labels: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse()?;
    let cases = load_dataset(&args.dataset)?;
    println!(
        "corpus:  {}\ndataset: {} ({} queries)\nprofiles: {}\n",
        args.codebase.display(),
        args.dataset.display(),
        cases.len(),
        args.profiles.join(", ")
    );

    let mut outcomes = Vec::new();
    for profile_name in &args.profiles {
        println!("--- {profile_name} ---");
        match run_profile(profile_name, &args, &cases).await {
            Ok(outcome) => {
                print_profile_detail(&outcome);
                outcomes.push(outcome);
            }
            // A profile that cannot run is reported as such and does NOT become
            // a zero row: "not measured" and "measured badly" are different
            // answers, and a zero would read as the model being terrible.
            Err(err) => println!("SKIPPED: {err:#}\n"),
        }
    }

    print_table(&outcomes, &args.profiles);

    // The table is printed either way — an hour of indexing should not be
    // thrown away over a bad label — but the verdict is machine-readable:
    // a broken ruler exits non-zero instead of only saying so on screen.
    if outcomes.iter().any(|o| !o.unreachable_labels.is_empty()) {
        std::process::exit(2);
    }
    Ok(())
}

async fn run_profile(
    profile_name: &str,
    args: &Args,
    cases: &[RelevanceCase],
) -> Result<ProfileOutcome> {
    let profile = resolve_profile(profile_name, &args.codebase)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let backend = EmbeddingBackend::from_profile(profile);
    let identity = backend.identity();

    // Everything this run writes lives in a scratch directory: the vector store
    // is derived from the cache path, so a repo-local cache would leave a
    // several-hundred-megabyte index per profile behind.
    let scratch = tempfile::TempDir::new().context("cannot create scratch dir")?;
    let cache = scratch.path().join("cache");
    let tantivy = scratch.path().join("tantivy");

    let mut indexer = UnifiedIndexer::for_embedded_with_backend(
        &cache,
        &tantivy,
        &format!("relevance_{}", profile_name.replace('-', "_")),
        backend.dim(),
        identity.as_str(),
        None,
        backend.clone(),
    )
    .await
    .context("cannot initialise indexer")?;

    let started = Instant::now();
    let stats = indexer
        .index_directory_parallel(&args.codebase)
        .await
        .context("indexing failed")?;
    let index_secs = started.elapsed().as_secs_f64();

    if stats.total_chunks == 0 {
        bail!("indexing produced no chunks; nothing to search");
    }

    // A label the indexer never ingested can never be found — by ANY profile.
    // Left unchecked it lowers every row by the same amount and reads as "the
    // models are weak" instead of "the ruler points at a file that is not
    // there". Caught here, where the actual index can be asked.
    let indexed = indexer
        .vector_store_cloned()
        .indexed_file_paths()
        .await
        .map_err(|e| anyhow::anyhow!("cannot list indexed files: {e}"))?;
    let indexed: std::collections::HashSet<RepoRelPath> = indexed
        .iter()
        .filter_map(|path| {
            RepoRelPath::from_indexed_path(Path::new(path), &args.codebase)
        })
        .collect();
    let unreachable_labels: Vec<String> = cases
        .iter()
        .flat_map(|case| case.expected_files.iter().map(move |f| (case, f)))
        .filter(|(_, file)| !indexed.contains(*file))
        .map(|(case, file)| format!("`{}` -> {}", case.query, file.0))
        .collect();

    // Same generator and same store for both modes — the ONLY difference is
    // whether BM25 participates. Anything else would confound the comparison.
    let vector_only = HybridSearch::with_defaults(
        indexer
            .embedding_generator_cloned()
            .map_err(|e| anyhow::anyhow!("{e}"))?,
        indexer.vector_store_cloned(),
        None,
    );
    let hybrid = HybridSearch::with_defaults(
        indexer
            .embedding_generator_cloned()
            .map_err(|e| anyhow::anyhow!("{e}"))?,
        indexer.vector_store_cloned(),
        Some(
            indexer
                .create_bm25_search()
                .map_err(|e| anyhow::anyhow!("{e}"))?,
        ),
    );

    let vector = evaluate(&vector_only, cases, &args.codebase).await?;
    let hybrid = evaluate(&hybrid, cases, &args.codebase).await?;

    Ok(ProfileOutcome {
        profile: profile_name.to_string(),
        unreachable_labels,
        dim: backend.dim(),
        indexed_files: stats.indexed_files,
        total_chunks: stats.total_chunks,
        index_secs,
        vector,
        hybrid,
    })
}

/// Run the whole query set through one search mode.
async fn evaluate(
    search: &HybridSearch,
    cases: &[RelevanceCase],
    repo_root: &Path,
) -> Result<RelevanceSummary> {
    let mut scores = Vec::with_capacity(cases.len());
    let mut unjudgeable = 0usize;

    for case in cases {
        // `SearchError` is not `Sync`, so it cannot ride anyhow's `Context`;
        // flattened to text here rather than propagated as a typed cause.
        let results = search
            .search(&case.query, RESULT_LIMIT)
            .await
            .map_err(|e| anyhow::anyhow!("search failed for query `{}`: {e}", case.query))?;

        let mut ranked = Vec::with_capacity(results.len());
        for result in &results {
            match RepoRelPath::from_indexed_path(&result.chunk.context.file_path, repo_root) {
                Some(path) => ranked.push(path),
                // A result from outside the corpus cannot be judged against a
                // dataset labelled inside it. Counted, never silently dropped.
                None => unjudgeable += 1,
            }
        }

        scores.push(score_case(case, &ranked).map_err(|e| anyhow::anyhow!("{e}"))?);
    }

    Ok(RelevanceSummary::from_scores(&scores, unjudgeable))
}

fn load_dataset(path: &Path) -> Result<Vec<RelevanceCase>> {
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read dataset {}", path.display()))?;
    let dataset: RelevanceDataset = serde_json::from_str(&source)
        .with_context(|| format!("cannot parse dataset {}", path.display()))?;

    for case in &dataset.cases {
        case.validate().map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    Ok(dataset.cases)
}

fn print_profile_detail(outcome: &ProfileOutcome) {
    println!(
        "indexed {} files / {} chunks in {:.1}s (dim {})",
        outcome.indexed_files, outcome.total_chunks, outcome.index_secs, outcome.dim
    );
    for (mode, summary) in [("vector", &outcome.vector), ("hybrid", &outcome.hybrid)] {
        println!(
            "  {mode:<7} recall@5={:.3} recall@10={:.3} mrr={:.3} \
             (queries={}, no hit at all={}, unjudgeable results={})",
            summary.mean_recall_at_5,
            summary.mean_recall_at_10,
            summary.mrr,
            summary.queries,
            summary.queries_without_hit,
            summary.unjudgeable_results
        );
    }
    if !outcome.unreachable_labels.is_empty() {
        println!(
            "  BROKEN RULER: {} label(s) name a file the indexer never ingested:",
            outcome.unreachable_labels.len()
        );
        for label in &outcome.unreachable_labels {
            println!("    {label}");
        }
    }
    println!();
}

fn print_table(outcomes: &[ProfileOutcome], requested: &[String]) {
    println!("\n=== profile x metric ===\n");
    println!(
        "{:<26} {:>5} {:>7} {:>9} {:>10} {:>7} {:>9}",
        "profile", "dim", "mode", "recall@5", "recall@10", "mrr", "no-hit"
    );
    for outcome in outcomes {
        for (mode, summary) in [("vector", &outcome.vector), ("hybrid", &outcome.hybrid)] {
            println!(
                "{:<26} {:>5} {:>7} {:>9.3} {:>10.3} {:>7.3} {:>4}/{:<4}",
                outcome.profile,
                outcome.dim,
                mode,
                summary.mean_recall_at_5,
                summary.mean_recall_at_10,
                summary.mrr,
                summary.queries_without_hit,
                summary.queries
            );
        }
    }

    // Coverage of the TABLE itself: a profile that never ran must not be
    // mistaken for one that ran and lost.
    let measured: Vec<&str> = outcomes.iter().map(|o| o.profile.as_str()).collect();
    let skipped: Vec<&str> = requested
        .iter()
        .map(String::as_str)
        .filter(|name| !measured.contains(name))
        .collect();
    println!(
        "\nmeasured {}/{} requested profiles",
        measured.len(),
        requested.len()
    );
    if !skipped.is_empty() {
        println!("NOT measured (no row above): {}", skipped.join(", "));
    }
}

#[derive(Debug)]
struct Args {
    codebase: PathBuf,
    dataset: PathBuf,
    profiles: Vec<String>,
}

impl Args {
    fn parse() -> Result<Self> {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();

        let mut codebase = repo_root.clone();
        let mut dataset = repo_root.join("eval/relevance_queries.json");
        let mut profiles = Vec::new();
        let mut args = std::env::args().skip(1);

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-h" | "--help" => {
                    println!(
                        "Usage: profile_relevance [--profile NAME]... [--codebase PATH] [--dataset PATH]"
                    );
                    std::process::exit(0);
                }
                "--codebase" => {
                    codebase = PathBuf::from(
                        args.next().context("--codebase requires a path")?,
                    );
                }
                "--dataset" => {
                    dataset =
                        PathBuf::from(args.next().context("--dataset requires a path")?);
                }
                "--profile" => {
                    profiles.push(args.next().context("--profile requires a name")?);
                }
                other => bail!("unknown argument `{other}`"),
            }
        }

        if profiles.is_empty() {
            profiles.push("local-cpu-small".to_string());
        }

        Ok(Self {
            codebase,
            dataset,
            profiles,
        })
    }
}
