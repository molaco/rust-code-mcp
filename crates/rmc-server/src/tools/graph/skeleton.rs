//! `crate_skeleton` endpoint and filesystem writer.

use std::fs;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use rmc_graph::graph::{
    SkeletonOptions, render_crate_skeletons,
};
use crate::tools::graph::response::*;
use crate::tools::params::CrateSkeletonParams;

use rmcp::{ErrorData as McpError, model::CallToolResult};

pub(crate) async fn crate_skeleton(
    params: CrateSkeletonParams,
) -> Result<CallToolResult, McpError> {
    validate_include(params.include.as_deref())?;

    let canonical = PathBuf::from(&params.directory)
        .canonicalize()
        .map_err(|e| {
            McpError::invalid_params(
                format!("failed to canonicalize {}: {e}", params.directory),
                None,
            )
        })?;
    let skeleton_dir = canonical.join(".skeleton");
    ensure_exact_skeleton_child(&canonical, &skeleton_dir)?;

    let directory = canonical.to_string_lossy().into_owned();
    let skeleton_dir_for_response = skeleton_dir.display().to_string();
    let clean = params.clean.unwrap_or(true);
    let page_req = list_page(&params.pagination);
    let opts = graph_options(&params);

    let response = tokio::task::spawn_blocking(move || {
        let snap = open_workspace_snapshot(&directory)?;
        if clean && skeleton_dir.exists() {
            fs::remove_dir_all(&skeleton_dir).map_err(|e| {
                McpError::internal_error(
                    format!("remove {}: {e}", skeleton_dir.display()),
                    None,
                )
            })?;
        }
        let output = render_crate_skeletons(&snap, &opts)
            .map_err(internal_error("render_crate_skeletons"))?;

        let mut summaries = Vec::new();
        for file in output.files {
            let relative = safe_relative_source_path(&file.source_path)?;
            let output_path = skeleton_dir.join(relative);
            if let Some(parent) = output_path.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    McpError::internal_error(
                        format!("create directory {}: {e}", parent.display()),
                        None,
                    )
                })?;
            }
            fs::write(&output_path, file.content.as_bytes()).map_err(|e| {
                McpError::internal_error(
                    format!("write {}: {e}", output_path.display()),
                    None,
                )
            })?;
            summaries.push(CrateSkeletonFileSummary {
                crate_name: file.crate_name,
                source_path: file.source_path,
                skeleton_path: file.skeleton_path,
                bytes: file.bytes,
                items: file.items,
            });
        }

        let (page, files_written) = if page_req.summary {
            (
                ListMeta {
                    total_match_count: summaries.len(),
                    offset: page_req.offset,
                    limit: page_req.limit,
                    summary: true,
                    returned_match_count: 0,
                },
                Vec::new(),
            )
        } else {
            page_list(summaries, page_req)
        };

        Ok(CrateSkeletonResponse {
            skeleton_dir: skeleton_dir_for_response,
            snapshot_id: output.snapshot_id,
            page,
            files_written,
            total_files: output.total_files,
            total_items: output.total_items,
            total_bytes: output.total_bytes,
            diagnostics: output
                .diagnostics
                .into_iter()
                .map(|diagnostic| diagnostic.message)
                .collect(),
        })
    })
    .await
    .map_err(|e| McpError::internal_error(format!("spawn_blocking join error: {e}"), None))??;

    json_result(&response)
}

#[derive(Debug, Serialize)]
pub(crate) struct CrateSkeletonResponse {
    pub(crate) skeleton_dir: String,
    pub(crate) snapshot_id: String,
    pub(crate) page: ListMeta,
    pub(crate) files_written: Vec<CrateSkeletonFileSummary>,
    pub(crate) total_files: usize,
    pub(crate) total_items: usize,
    pub(crate) total_bytes: usize,
    pub(crate) diagnostics: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CrateSkeletonFileSummary {
    pub(crate) crate_name: String,
    pub(crate) source_path: String,
    pub(crate) skeleton_path: String,
    pub(crate) bytes: usize,
    pub(crate) items: usize,
}

fn graph_options(params: &CrateSkeletonParams) -> SkeletonOptions {
    let defaults = SkeletonOptions::default();
    SkeletonOptions {
        crates: params.crates.clone(),
        include: params
            .include
            .clone()
            .unwrap_or(defaults.include),
        include_docs: params.include_docs.unwrap_or(defaults.include_docs),
        include_attrs: params.include_attrs.unwrap_or(defaults.include_attrs),
        include_impls: params.include_impls.unwrap_or(defaults.include_impls),
        skip_test_items: params
            .skip_test_items
            .unwrap_or(defaults.skip_test_items),
        exclude_vendor: params.exclude_vendor.unwrap_or(defaults.exclude_vendor),
    }
}

fn validate_include(include: Option<&[String]>) -> Result<(), McpError> {
    let Some(include) = include else {
        return Ok(());
    };
    for value in include {
        match value.as_str() {
            "pub" | "pub(crate)" | "restricted" | "private" | "all" => {}
            other => {
                return Err(McpError::invalid_params(
                    format!(
                        "unknown skeleton include value `{other}`; expected pub | pub(crate) | restricted | private | all"
                    ),
                    None,
                ));
            }
        }
    }
    Ok(())
}

fn ensure_exact_skeleton_child(root: &Path, skeleton_dir: &Path) -> Result<(), McpError> {
    if skeleton_dir.parent() != Some(root) || skeleton_dir.file_name() != Some(OsStr::new(".skeleton"))
    {
        return Err(McpError::invalid_params(
            format!(
                "refusing to write skeleton outside exact generated directory: {}",
                skeleton_dir.display()
            ),
            None,
        ));
    }
    Ok(())
}

fn safe_relative_source_path(source_path: &str) -> Result<&Path, McpError> {
    let path = Path::new(source_path);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_))
        })
    {
        return Err(McpError::internal_error(
            format!("unsafe skeleton source path `{source_path}`"),
            None,
        ));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_relative_source_path_rejects_escape_paths() {
        assert!(safe_relative_source_path("crates/rmc-server/src/lib.rs").is_ok());
        assert!(safe_relative_source_path("../src/lib.rs").is_err());
        assert!(safe_relative_source_path("/tmp/lib.rs").is_err());
    }
}
