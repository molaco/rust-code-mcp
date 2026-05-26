//! `crate_skeleton` endpoint and filesystem writer.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs, io,
    path::{Component, Path, PathBuf},
};

use serde::Serialize;

use rmc_graph::graph::{
    SkeletonFile, SkeletonOptions, render_crate_skeletons,
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
        let snapshot_id = output.snapshot_id.clone();

        let mut summaries = Vec::new();
        let mut aggregates: BTreeMap<String, CrateSkeletonAggregateBuilder> = BTreeMap::new();
        for file in output.files {
            let relative = safe_relative_source_path(&file.source_path)?;
            write_skeleton_file(&skeleton_dir, relative, file.content.as_bytes())?;
            push_aggregate_file(&mut aggregates, &file)?;
            summaries.push(CrateSkeletonFileSummary {
                crate_name: file.crate_name,
                source_path: file.source_path,
                skeleton_path: file.skeleton_path,
                bytes: file.bytes,
                items: file.items,
            });
        }
        let aggregate_summaries =
            write_aggregate_files(&skeleton_dir, &snapshot_id, aggregates)?;
        let total_aggregate_files = aggregate_summaries.len();
        let total_aggregate_bytes = aggregate_summaries
            .iter()
            .map(|summary| summary.bytes)
            .sum();

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
        let aggregate_files_written = if page_req.summary {
            Vec::new()
        } else {
            aggregate_summaries
        };

        Ok(CrateSkeletonResponse {
            skeleton_dir: skeleton_dir_for_response,
            snapshot_id,
            page,
            files_written,
            aggregate_files_written,
            total_files: output.total_files,
            total_items: output.total_items,
            total_bytes: output.total_bytes,
            total_aggregate_files,
            total_aggregate_bytes,
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
    pub(crate) aggregate_files_written: Vec<CrateSkeletonAggregateFileSummary>,
    pub(crate) total_files: usize,
    pub(crate) total_items: usize,
    pub(crate) total_bytes: usize,
    pub(crate) total_aggregate_files: usize,
    pub(crate) total_aggregate_bytes: usize,
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

#[derive(Debug, Serialize)]
pub(crate) struct CrateSkeletonAggregateFileSummary {
    pub(crate) crate_names: Vec<String>,
    pub(crate) skeleton_path: String,
    pub(crate) source_files: usize,
    pub(crate) bytes: usize,
    pub(crate) items: usize,
}

#[derive(Debug)]
struct CrateSkeletonAggregateBuilder {
    file_name: String,
    crate_names: BTreeSet<String>,
    source_files: Vec<CrateSkeletonAggregateSource>,
    items: usize,
}

#[derive(Debug)]
struct CrateSkeletonAggregateSource {
    source_path: String,
    content: String,
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

fn push_aggregate_file(
    aggregates: &mut BTreeMap<String, CrateSkeletonAggregateBuilder>,
    file: &SkeletonFile,
) -> Result<(), McpError> {
    let file_name = aggregate_file_name(&file.crate_name, &file.source_path)?;
    let entry = aggregates
        .entry(file_name.clone())
        .or_insert_with(|| CrateSkeletonAggregateBuilder {
            file_name,
            crate_names: BTreeSet::new(),
            source_files: Vec::new(),
            items: 0,
        });
    entry.crate_names.insert(file.crate_name.clone());
    entry.items += file.items;
    entry.source_files.push(CrateSkeletonAggregateSource {
        source_path: file.source_path.clone(),
        content: file.content.clone(),
    });
    Ok(())
}

fn aggregate_file_name(crate_name: &str, source_path: &str) -> Result<String, McpError> {
    let stem = package_name_from_source_path(source_path).unwrap_or(crate_name);
    if stem.is_empty()
        || stem == "."
        || stem == ".."
        || stem.contains('/')
        || stem.contains('\\')
        || stem.contains('\0')
    {
        return Err(McpError::internal_error(
            format!("unsafe aggregate skeleton file stem `{stem}`"),
            None,
        ));
    }
    Ok(format!("{stem}.rs"))
}

fn package_name_from_source_path(source_path: &str) -> Option<&str> {
    let mut components = Path::new(source_path).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(root)), Some(Component::Normal(package))) => {
            if root == OsStr::new("crates") {
                package.to_str()
            } else {
                None
            }
        }
        _ => None,
    }
}

fn write_aggregate_files(
    skeleton_dir: &Path,
    snapshot_id: &str,
    aggregates: BTreeMap<String, CrateSkeletonAggregateBuilder>,
) -> Result<Vec<CrateSkeletonAggregateFileSummary>, McpError> {
    let mut summaries = Vec::new();
    for (_file_name, aggregate) in aggregates {
        let content = render_aggregate_file(snapshot_id, &aggregate);
        let bytes = content.len();
        let source_files = aggregate.source_files.len();
        let items = aggregate.items;
        let crate_names = aggregate.crate_names.into_iter().collect();
        write_skeleton_file(
            skeleton_dir,
            Path::new(&aggregate.file_name),
            content.as_bytes(),
        )?;
        summaries.push(CrateSkeletonAggregateFileSummary {
            crate_names,
            skeleton_path: format!(".skeleton/{}", aggregate.file_name),
            source_files,
            bytes,
            items,
        });
    }
    Ok(summaries)
}

fn render_aggregate_file(
    snapshot_id: &str,
    aggregate: &CrateSkeletonAggregateBuilder,
) -> String {
    let mut content = String::new();
    content.push_str("// @generated by rust-code-mcp crate_skeleton\n");
    content.push_str(&format!("// snapshot: {snapshot_id}\n"));
    content.push_str(&format!("// aggregate: {}\n", aggregate.file_name));
    content.push_str(&format!(
        "// crates: {}\n",
        aggregate
            .crate_names
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(",")
    ));
    content.push('\n');

    for source in &aggregate.source_files {
        content.push_str(&format!("// ---- source: {} ----\n", source.source_path));
        content.push_str(source.content.trim_end());
        content.push_str("\n\n");
    }

    content
}

fn write_skeleton_file(
    skeleton_dir: &Path,
    relative: &Path,
    content: &[u8],
) -> Result<(), McpError> {
    let output_path = skeleton_dir.join(relative);
    let parent = output_path
        .parent()
        .filter(|parent| parent.starts_with(skeleton_dir))
        .ok_or_else(|| {
            McpError::internal_error(
                format!(
                    "refusing to write skeleton outside exact generated directory: {}",
                    output_path.display()
                ),
                None,
            )
        })?;

    create_skeleton_parent_dirs(skeleton_dir, parent)?;
    ensure_output_file_is_not_symlink(&output_path)?;
    fs::write(&output_path, content).map_err(|e| {
        McpError::internal_error(
            format!("write {}: {e}", output_path.display()),
            None,
        )
    })
}

fn create_skeleton_parent_dirs(skeleton_dir: &Path, parent: &Path) -> Result<(), McpError> {
    ensure_existing_skeleton_ancestors_not_symlinks(skeleton_dir, parent)?;
    fs::create_dir_all(parent).map_err(|e| {
        McpError::internal_error(
            format!("create directory {}: {e}", parent.display()),
            None,
        )
    })?;
    ensure_existing_skeleton_ancestors_not_symlinks(skeleton_dir, parent)
}

fn ensure_existing_skeleton_ancestors_not_symlinks(
    skeleton_dir: &Path,
    parent: &Path,
) -> Result<(), McpError> {
    let relative_parent = parent.strip_prefix(skeleton_dir).map_err(|_| {
        McpError::internal_error(
            format!(
                "refusing to inspect skeleton ancestor outside generated directory: {}",
                parent.display()
            ),
            None,
        )
    })?;

    let mut current = skeleton_dir.to_path_buf();
    inspect_existing_skeleton_ancestor(&current)?;
    for component in relative_parent.components() {
        match component {
            Component::Normal(name) => {
                current.push(name);
                inspect_existing_skeleton_ancestor(&current)?;
            }
            Component::CurDir => {}
            _ => {
                return Err(McpError::internal_error(
                    format!(
                        "refusing to inspect unsafe skeleton ancestor: {}",
                        parent.display()
                    ),
                    None,
                ));
            }
        }
    }

    Ok(())
}

fn inspect_existing_skeleton_ancestor(path: &Path) -> Result<(), McpError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(McpError::internal_error(
                format!(
                    "refusing to write through symlinked skeleton path: {}",
                    path.display()
                ),
                None,
            ))
        }
        Ok(metadata) if !metadata.file_type().is_dir() => {
            Err(McpError::internal_error(
                format!(
                    "refusing to write through non-directory skeleton path: {}",
                    path.display()
                ),
                None,
            ))
        }
        Ok(_) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(McpError::internal_error(
            format!("inspect skeleton path {}: {err}", path.display()),
            None,
        )),
    }
}

fn ensure_output_file_is_not_symlink(output_path: &Path) -> Result<(), McpError> {
    match fs::symlink_metadata(output_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(McpError::internal_error(
                format!(
                    "refusing to overwrite symlinked skeleton file: {}",
                    output_path.display()
                ),
                None,
            ))
        }
        Ok(_) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(McpError::internal_error(
            format!("inspect skeleton file {}: {err}", output_path.display()),
            None,
        )),
    }
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

    #[test]
    fn aggregate_file_name_prefers_package_directory_name() {
        assert_eq!(
            aggregate_file_name("rmc_config", "crates/rmc-config/src/config.rs")
                .expect("aggregate file name"),
            "rmc-config.rs"
        );
        assert_eq!(
            aggregate_file_name("rmc_config", "src/config.rs")
                .expect("aggregate file name"),
            "rmc_config.rs"
        );
    }

    #[test]
    fn write_aggregate_files_concatenates_by_package_name() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let skeleton_dir = temp_dir.path().join("workspace/.skeleton");
        let content = "pub struct Demo;\n".to_string();
        let file = SkeletonFile {
            crate_name: "rmc_server".to_string(),
            source_path: "crates/rmc-server/src/lib.rs".to_string(),
            skeleton_path: ".skeleton/crates/rmc-server/src/lib.rs".to_string(),
            bytes: content.len(),
            content,
            items: 1,
        };
        let mut aggregates = BTreeMap::new();
        push_aggregate_file(&mut aggregates, &file).expect("push aggregate file");

        let summaries =
            write_aggregate_files(&skeleton_dir, "snapshot-id", aggregates)
                .expect("write aggregate files");

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].skeleton_path, ".skeleton/rmc-server.rs");
        assert_eq!(summaries[0].crate_names, vec!["rmc_server".to_string()]);
        assert_eq!(summaries[0].source_files, 1);
        assert_eq!(summaries[0].items, 1);
        let written = skeleton_dir.join("rmc-server.rs");
        let generated = fs::read_to_string(&written).expect("read aggregate file");
        assert!(generated.contains("// aggregate: rmc-server.rs"));
        assert!(generated.contains("// ---- source: crates/rmc-server/src/lib.rs ----"));
        assert!(generated.contains("pub struct Demo;"));
    }

    #[test]
    fn write_skeleton_file_creates_normal_paths() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let workspace = temp_dir.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace dir");
        let skeleton_dir = workspace.join(".skeleton");

        write_skeleton_file(
            &skeleton_dir,
            Path::new("crates/demo/src/lib.rs"),
            b"pub struct Demo;\n",
        )
        .expect("write skeleton file");

        let written = skeleton_dir.join("crates/demo/src/lib.rs");
        assert_eq!(
            fs::read_to_string(&written).expect("read skeleton file"),
            "pub struct Demo;\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_skeleton_file_rejects_symlinked_skeleton_root() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().expect("temp dir");
        let workspace = temp_dir.path().join("workspace");
        let escaped = temp_dir.path().join("escaped");
        fs::create_dir_all(&workspace).expect("workspace dir");
        fs::create_dir_all(&escaped).expect("escaped dir");
        let skeleton_dir = workspace.join(".skeleton");
        symlink(&escaped, &skeleton_dir).expect("skeleton root symlink");

        let err = write_skeleton_file(
            &skeleton_dir,
            Path::new("crates/demo/src/lib.rs"),
            b"pub struct Demo;\n",
        )
        .expect_err("symlinked skeleton root should be rejected");

        assert!(
            err.message.contains("symlinked skeleton path"),
            "unexpected error: {}",
            err.message
        );
        assert!(!escaped.join("crates/demo/src/lib.rs").exists());
    }

    #[cfg(unix)]
    #[test]
    fn write_skeleton_file_rejects_symlinked_existing_ancestor() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().expect("temp dir");
        let workspace = temp_dir.path().join("workspace");
        let escaped = temp_dir.path().join("escaped");
        fs::create_dir_all(workspace.join(".skeleton")).expect("skeleton dir");
        fs::create_dir_all(&escaped).expect("escaped dir");
        let skeleton_dir = workspace.join(".skeleton");
        symlink(&escaped, skeleton_dir.join("crates")).expect("ancestor symlink");

        let err = write_skeleton_file(
            &skeleton_dir,
            Path::new("crates/demo/src/lib.rs"),
            b"pub struct Demo;\n",
        )
        .expect_err("symlinked skeleton ancestor should be rejected");

        assert!(
            err.message.contains("symlinked skeleton path"),
            "unexpected error: {}",
            err.message
        );
        assert!(!escaped.join("demo/src/lib.rs").exists());
    }

    #[cfg(unix)]
    #[test]
    fn write_skeleton_file_rejects_symlink_output_file() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().expect("temp dir");
        let workspace = temp_dir.path().join("workspace");
        let skeleton_dir = workspace.join(".skeleton");
        let parent = skeleton_dir.join("crates/demo/src");
        let outside_file = temp_dir.path().join("outside.rs");
        let output_file = parent.join("lib.rs");
        fs::create_dir_all(&parent).expect("skeleton parent dir");
        fs::write(&outside_file, "outside\n").expect("outside file");
        symlink(&outside_file, &output_file).expect("output file symlink");

        let err = write_skeleton_file(
            &skeleton_dir,
            Path::new("crates/demo/src/lib.rs"),
            b"pub struct Demo;\n",
        )
        .expect_err("symlinked output file should be rejected");

        assert!(
            err.message.contains("symlinked skeleton file"),
            "unexpected error: {}",
            err.message
        );
        assert_eq!(
            fs::read_to_string(&outside_file).expect("read outside file"),
            "outside\n"
        );
        assert!(
            fs::symlink_metadata(&output_file)
                .expect("output symlink metadata")
                .file_type()
                .is_symlink()
        );
    }
}
