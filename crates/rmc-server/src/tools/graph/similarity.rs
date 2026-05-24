//! Similarity endpoint family.
//!
//! Similarity endpoint family.
//!
//! `similar_to_item` remains server-owned because it depends on server
//! project-path and hybrid-search policy. Workspace-wide `semantic_overlaps`
//! delegates graph item enumeration, embedding-cache use, cosine scoring, and
//! clustering to the graph-owned similarity facade.

use std::path::{Path, PathBuf};

use rmcp::{ErrorData as McpError, model::CallToolResult};
use serde::Serialize;

use rmc_graph::graph::{
    GraphSimilarityError, SemanticOverlapOptions, item_kind_short_label as short_item_kind_label,
    run_semantic_overlaps,
};
use crate::mcp::project_paths::resolve_embedding_backend_for_mcp;
use crate::tools::graph::response::*;
use crate::tools::params::{SemanticOverlapsParams, SimilarToItemParams};

/// v0.1 "semantic overlaps": resolve `target` to a hypergraph Item, read its
/// source bytes from (file, span), and run vector_only_search using those
/// bytes as the query. Drops the seed's own chunk (file-path-only match — see
/// limitation note) and applies optional `threshold` / `item_kind` filters.
///
/// Limitation: self-match detection is file-path-only. If the seed file
/// contains other items that match the seed's source semantically, those
/// will be returned. A finer span-overlap check is left for v0.2.
pub(crate) async fn similar_to_item(
    params: SimilarToItemParams,
) -> Result<CallToolResult, McpError> {
    // 1. Resolve seed Item from the hypergraph snapshot.
    let snap = open_workspace_snapshot(&params.directory)?;
    let (_seed_id, seed_node) = snap
        .lookup_by_qualified_name(&params.target)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.target),
                None,
            )
        })?;

    let seed_file = seed_node.file.clone().ok_or_else(|| {
        McpError::invalid_params(
            format!(
                "target item `{}` has no source location (synthetic / macro-generated?)",
                params.target
            ),
            None,
        )
    })?;
    let seed_span = seed_node.span.ok_or_else(|| {
        McpError::invalid_params(
            format!(
                "target item `{}` has no source span (synthetic / macro-generated?)",
                params.target
            ),
            None,
        )
    })?;

    // 2. Read seed source bytes from disk.
    let abs_path = PathBuf::from(&params.directory).join(&seed_file);
    let content = std::fs::read_to_string(&abs_path).map_err(|e| {
        McpError::invalid_params(
            format!("failed to read seed file `{}`: {e}", abs_path.display()),
            None,
        )
    })?;
    let (start, end) = (seed_span.0 as usize, seed_span.1 as usize);
    let seed_source = content.get(start..end).ok_or_else(|| {
        McpError::invalid_params(
            format!(
                "seed span {}..{} is out of bounds or splits a UTF-8 character in `{}` (file len = {})",
                start,
                end,
                abs_path.display(),
                content.len()
            ),
            None,
        )
    })?.to_string();

    // 3. Run vector-only search against the index built with the
    //    requested embedding profile (the default profile when unset).
    let backend = resolve_embedding_backend_for_mcp(
        params.embedding_profile.as_deref(),
        Path::new(&params.directory),
    )?;
    let paths = crate::mcp::project_paths::ProjectPaths::from_directory(
        Path::new(&params.directory),
        &backend,
    );
    let hybrid_search =
        crate::tools::endpoints::query::create_hybrid_search(&paths, None, backend).await?;

    let limit = params.limit.unwrap_or(10);
    let threshold = params.threshold.unwrap_or(0.0);
    let limit_plus_one = limit.saturating_add(1);

    let results = hybrid_search
        .vector_only_search(&seed_source, limit_plus_one)
        .await
        .map_err(|e| McpError::invalid_params(format!("vector search failed: {e}"), None))?;

    // 4. Filter results.
    let item_kind_filter = params.item_kind.as_ref().map(|s| s.to_lowercase());
    // Precompute the seed's line range once for the self-match overlap check.
    // The seed_file is workspace-relative (e.g. `crates/foo/src/lib.rs`) but
    // chunk file paths from the vector store are absolute, so we use
    // `Path::ends_with` for the same-file check (component-aware suffix match,
    // not byte equality) — this avoids the v0.1 false-negative where the seed
    // appeared as the top match because the relative-vs-absolute paths never
    // compared equal as strings.
    let seed_line_start = content[..start].matches('\n').count() + 1;
    let seed_line_end = content[..end].matches('\n').count() + 1;
    let seed_rel_path = Path::new(&seed_file);
    let mut matches: Vec<SimilarMatch> = Vec::new();
    for r in results {
        if r.chunk.context.file_path.ends_with(seed_rel_path) {
            // Drop only chunks whose line range overlaps the seed's byte span,
            // not every chunk in the same file.
            let result_line_start = r.chunk.context.line_start;
            let result_line_end = r.chunk.context.line_end;
            let overlaps = result_line_start <= seed_line_end
                && result_line_end >= seed_line_start;
            if overlaps {
                continue;
            }
        }
        if r.score < threshold {
            continue;
        }
        if let Some(ref want) = item_kind_filter {
            if r.chunk.context.symbol_kind.to_lowercase() != *want {
                continue;
            }
        }
        let preview = r
            .chunk
            .content
            .lines()
            .take(3)
            .collect::<Vec<_>>()
            .join("\n");
        matches.push(SimilarMatch {
            similarity: r.score,
            symbol_name: r.chunk.context.symbol_name,
            symbol_kind: r.chunk.context.symbol_kind,
            file: r.chunk.context.file_path.to_string_lossy().to_string(),
            line_start: r.chunk.context.line_start,
            line_end: r.chunk.context.line_end,
            preview,
        });
        if matches.len() >= limit {
            break;
        }
    }

    // 6. Build response.
    let resp = SimilarToItemResp {
        seed: SeedItemRef {
            qualified_name: seed_node.qualified_name,
            file: seed_file,
            span: seed_span,
            item_kind: seed_node.item_kind.map(|k| short_item_kind_label(k).to_string()),
        },
        limit,
        threshold,
        item_kind_filter: params.item_kind,
        match_count: matches.len(),
        matches,
    };
    json_result(&resp)
}

pub(crate) async fn semantic_overlaps(
    params: SemanticOverlapsParams,
) -> Result<CallToolResult, McpError> {
    let directory = params.directory.clone();
    let backend = resolve_embedding_backend_for_mcp(
        params.embedding_profile.as_deref(),
        Path::new(&directory),
    )?;
    let output = run_semantic_overlaps(
        Path::new(&directory),
        &backend,
        SemanticOverlapOptions {
            threshold: params.threshold,
            max_pairs: params.max_pairs,
            offset: params.offset,
            summary: params.summary,
            max_cluster_size: params.max_cluster_size,
            output_mode: params.output_mode,
            skip_test_chunks: params.skip_test_chunks,
            cross_crate_only: params.cross_crate_only,
            item_kind: params.item_kind,
            crate_name: params.crate_name,
        },
    )
    .await
    .map_err(graph_similarity_error("semantic_overlaps"))?;
    json_result(&output)
}

// ----- response shapes -----

#[derive(Debug, Serialize)]
pub(crate) struct SimilarToItemResp {
    pub(crate) seed: SeedItemRef,
    pub(crate) limit: usize,
    pub(crate) threshold: f32,
    pub(crate) item_kind_filter: Option<String>,
    pub(crate) match_count: usize,
    pub(crate) matches: Vec<SimilarMatch>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SeedItemRef {
    pub(crate) qualified_name: String,
    pub(crate) file: String,
    pub(crate) span: (u32, u32),
    /// Short label form (e.g. `"Fn"`, `"Struct"`); `None` when the seed Node
    /// has no `item_kind` (e.g. it's a Module).
    pub(crate) item_kind: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SimilarMatch {
    pub(crate) similarity: f32,
    pub(crate) symbol_name: String,
    pub(crate) symbol_kind: String,
    pub(crate) file: String,
    pub(crate) line_start: usize,
    pub(crate) line_end: usize,
    /// First 3 lines of `chunk.content` joined with `\n`.
    pub(crate) preview: String,
}

fn graph_similarity_error(label: &'static str) -> impl FnOnce(anyhow::Error) -> McpError {
    move |error| {
        let message = format!("{error:#}");
        if error.downcast_ref::<GraphSimilarityError>().is_some() {
            McpError::invalid_params(message, None)
        } else {
            McpError::internal_error(format!("{label}: {message}"), None)
        }
    }
}
