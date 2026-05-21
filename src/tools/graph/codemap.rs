//! Codemap endpoint family.
//!
//! Bridge between the `#[tool]` method in `search_tool_router.rs` and the
//! algorithm core in `src/graph/codemap/build.rs`. Validates params, opens
//! the workspace snapshot, resolves seeds either via qualified-name lookup
//! or by running `HybridSearch::search`, maps the resulting hits into the
//! codemap-local `SeedHit` DTO, calls `build_codemap`, and serializes the
//! result per `format`.

use rmcp::{
    ErrorData as McpError,
    model::{CallToolResult, Content},
};

use crate::tools::graph::response::*;

/// Bridge between the `#[tool]` method in `search_tool_router.rs` and the
/// algorithm core in `src/graph/codemap/build.rs`.
///
/// Validates params, opens the workspace snapshot, resolves seeds either via
/// qualified-name lookup or by running `HybridSearch::search`, calls
/// `build_codemap`, and serializes the result per `format`.
///
/// Default caps: `max_nodes=80` (cap 500), `depth=3` (cap 5),
/// `max_incoming_per_node=8`, `embedding_policy="no_rerank"`,
/// `format="json"`, `include_snippets=false`. `top_k_seeds` is hardcoded to
/// the algorithm-core default (20).
pub(crate) async fn handle_build_codemap(
    directory: &str,
    task_prompt: Option<&str>,
    seed_qualified_names: Option<&[String]>,
    max_nodes: Option<usize>,
    depth: Option<u8>,
    max_incoming_per_node: Option<usize>,
    embedding_policy: Option<&str>,
    format: Option<&str>,
    include_snippets: Option<bool>,
) -> Result<CallToolResult, McpError> {
    use crate::graph::codemap::{
        CodemapOptions, EmbeddingPolicy, build_codemap, render_mermaid, render_outline,
    };

    // ---------- validate ----------
    let trimmed_prompt = task_prompt.map(str::trim).filter(|s| !s.is_empty());
    let has_seeds = seed_qualified_names.map_or(false, |s| !s.is_empty());
    if trimmed_prompt.is_none() && !has_seeds {
        return Err(McpError::invalid_params(
            "either `task_prompt` or non-empty `seed_qualified_names` must be provided",
            None,
        ));
    }

    let policy = match embedding_policy.map(str::trim).unwrap_or("no_rerank") {
        "no_rerank" => EmbeddingPolicy::NoRerank,
        "cached_only" => EmbeddingPolicy::UseCachedOnly,
        "compute_missing" => EmbeddingPolicy::ComputeMissing,
        other => {
            return Err(McpError::invalid_params(
                format!(
                    "unknown embedding_policy `{other}`; expected `no_rerank` | `cached_only` | `compute_missing`"
                ),
                None,
            ));
        }
    };

    let format_choice = format.map(str::trim).unwrap_or("json");
    if !matches!(format_choice, "json" | "mermaid" | "outline" | "all") {
        return Err(McpError::invalid_params(
            format!(
                "unknown format `{format_choice}`; expected `json` | `mermaid` | `outline` | `all`"
            ),
            None,
        ));
    }

    let max_nodes = max_nodes.unwrap_or(80).min(500).max(1);
    let depth = depth.unwrap_or(3).min(5);
    let max_incoming_per_node = max_incoming_per_node.unwrap_or(8);
    let include_snippets = include_snippets.unwrap_or(false);

    let opts = CodemapOptions {
        max_nodes,
        depth,
        top_k_seeds: 20,
        max_incoming_per_node,
        embedding_policy: policy,
        include_snippets,
    };

    // ---------- open snapshot ----------
    let snap = open_workspace_snapshot(directory)?;

    // ---------- pre-flight staleness check ----------
    // Compare the snapshot's `created_at_unix` against the newest `.rs`
    // mtime under the workspace. If sources are newer, surface a
    // diagnostic in the resulting Codemap so the caller knows to
    // re-run `build_hypergraph(force_rebuild=true)`.
    let mut pre_diagnostics: Vec<String> = Vec::new();
    {
        let workspace_root = std::path::Path::new(&snap.manifest.workspace_root);
        if let Some(newest) = crate::graph::codemap::newest_source_mtime(workspace_root) {
            let created = snap.manifest.created_at_unix;
            if newest > created {
                let age = newest - created;
                pre_diagnostics.push(format!(
                    "snapshot is older than newest .rs file; consider build_hypergraph(force_rebuild=true) (snapshot is {age} seconds older)"
                ));
            }
        }
    }

    // ---------- build ----------
    let codemap = if let Some(names) = seed_qualified_names.filter(|s| !s.is_empty()) {
        build_codemap(&snap, trimmed_prompt, Some(names), None, &opts, &pre_diagnostics)
            .await
            .map_err(internal_error("build_codemap"))?
    } else {
        // Path: HybridSearch::search. trimmed_prompt is Some here (else we'd
        // have errored above).
        let prompt = trimmed_prompt.expect("validated above");
        let dir_path = std::path::Path::new(directory);
        let paths = crate::tools::project_paths::ProjectPaths::from_directory(
            dir_path,
            &crate::embeddings::EmbeddingBackend::default(),
        );
        // Best-effort BM25 open. If absent we get vector-only hits; that is
        // still a valid seed source.
        let bm25 = {
            use crate::config::indexer::TantivyConfig;
            use crate::indexing::tantivy_adapter::TantivyAdapter;
            let config = TantivyConfig::default(&paths.tantivy_path);
            TantivyAdapter::new(config)
                .and_then(|adapter| adapter.create_bm25_search())
                .ok()
        };
        let hybrid = crate::tools::endpoints::query::create_hybrid_search(
            &paths,
            bm25,
            crate::embeddings::EmbeddingBackend::default(),
        )
        .await?;
        let hits = hybrid
            .search(prompt, opts.top_k_seeds.saturating_mul(3))
            .await
            .map_err(|e| McpError::internal_error(format!("hybrid search: {e}"), None))?;
        let seed_hits = search_results_to_seed_hits(&hits);
        build_codemap(&snap, Some(prompt), None, Some(&seed_hits), &opts, &pre_diagnostics)
            .await
            .map_err(internal_error("build_codemap"))?
    };

    // ---------- format ----------
    match format_choice {
        "json" => json_result(&codemap),
        "mermaid" => Ok(CallToolResult::success(vec![Content::text(render_mermaid(
            &codemap,
        ))])),
        "outline" => Ok(CallToolResult::success(vec![Content::text(render_outline(
            &codemap,
        ))])),
        "all" => {
            let payload = serde_json::json!({
                "json": &codemap,
                "mermaid": render_mermaid(&codemap),
                "outline": render_outline(&codemap),
            });
            let s = serde_json::to_string_pretty(&payload)
                .map_err(|e| McpError::internal_error(format!("serialize: {e}"), None))?;
            Ok(CallToolResult::success(vec![Content::text(s)]))
        }
        _ => unreachable!("format validated above"),
    }
}

/// Adapt the search layer's `SearchResult` slice into the codemap-local
/// `SeedHit` slice. Keeping this mapping on the tools side is what lets the
/// `graph::codemap` algorithm core stay search-independent (PR 12 boundary
/// fix).
fn search_results_to_seed_hits(
    results: &[crate::search::SearchResult],
) -> Vec<crate::graph::codemap::SeedHit> {
    results
        .iter()
        .map(|r| crate::graph::codemap::SeedHit {
            file_path: r.chunk.context.file_path.clone(),
            line_start: r.chunk.context.line_start as u32,
            line_end: r.chunk.context.line_end as u32,
            score: r.score,
        })
        .collect()
}
