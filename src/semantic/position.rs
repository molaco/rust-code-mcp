//! Position and coordinate utilities

use std::path::{Path, PathBuf};
use ra_ap_ide::{AnalysisHost, FilePosition, LineCol, NavigationTarget, TextSize};
use ra_ap_vfs::{Vfs, VfsPath};
use anyhow::{Result, Context};

/// A source code location
#[derive(Debug, Clone)]
pub struct Location {
    pub file_path: PathBuf,
    pub line: u32,      // 1-based
    pub column: u32,    // 1-based
    pub name: String,
}

impl std::fmt::Display for Location {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{} ({})", self.file_path.display(), self.line, self.column, self.name)
    }
}

/// Convert file path to FileId
fn path_to_file_id(vfs: &Vfs, file_path: &Path) -> Result<ra_ap_vfs::FileId> {
    let abs_path = file_path.canonicalize()
        .context("Failed to canonicalize path")?;
    let vfs_path = VfsPath::new_real_path(abs_path.to_string_lossy().to_string());

    vfs.file_id(&vfs_path)
        .map(|(id, _)| id)
        .ok_or_else(|| anyhow::anyhow!("File not found in VFS: {}", file_path.display()))
}

/// Convert line/column to byte offset
fn to_offset(
    analysis: &ra_ap_ide::Analysis,
    file_id: ra_ap_vfs::FileId,
    line: u32,
    column: u32,
) -> Result<TextSize> {
    let line_index = analysis.file_line_index(file_id)
        .context("Failed to get line index")?;

    // LineCol is 0-based, input is 1-based
    let line_col = LineCol {
        line: line.saturating_sub(1),
        col: column.saturating_sub(1),
    };

    line_index.offset(line_col)
        .ok_or_else(|| anyhow::anyhow!("Invalid position: line {}, col {}", line, column))
}

/// Convert NavigationTarget to Location
fn nav_target_to_location(
    vfs: &Vfs,
    analysis: &ra_ap_ide::Analysis,
    target: &NavigationTarget,
) -> Result<Location> {
    let vfs_path = vfs.file_path(target.file_id);
    let file_path: PathBuf = vfs_path.as_path()
        .ok_or_else(|| anyhow::anyhow!("Not a real path"))?
        .to_path_buf()
        .into();

    let line_index = analysis.file_line_index(target.file_id)?;
    let offset = target.focus_range.unwrap_or(target.full_range).start();
    let line_col = line_index.line_col(offset);

    Ok(Location {
        file_path,
        line: line_col.line + 1,
        column: line_col.col + 1,
        name: target.name.to_string(),
    })
}

/// Goto definition at position
pub fn goto_definition(
    host: &AnalysisHost,
    vfs: &Vfs,
    file_path: &Path,
    line: u32,
    column: u32,
) -> Result<Vec<Location>> {
    let analysis = host.analysis();
    let file_id = path_to_file_id(vfs, file_path)?;
    let offset = to_offset(&analysis, file_id, line, column)?;

    let position = FilePosition { file_id, offset };
    let config = ra_ap_ide::GotoDefinitionConfig { minicore: Default::default() };

    let result = analysis.goto_definition(position, &config)
        .context("goto_definition query failed")?;

    match result {
        Some(nav_info) => {
            nav_info.info
                .iter()
                .map(|target| nav_target_to_location(vfs, &analysis, target))
                .collect()
        }
        None => Ok(vec![]),
    }
}

/// Find all references at position
pub fn find_references(
    host: &AnalysisHost,
    vfs: &Vfs,
    file_path: &Path,
    line: u32,
    column: u32,
) -> Result<Vec<Location>> {
    let analysis = host.analysis();
    let file_id = path_to_file_id(vfs, file_path)?;
    let offset = to_offset(&analysis, file_id, line, column)?;

    let position = FilePosition { file_id, offset };
    let config = ra_ap_ide::FindAllRefsConfig {
        minicore: Default::default(),
        search_scope: None,
    };

    let results = analysis.find_all_refs(position, &config)
        .context("find_all_refs query failed")?;

    let mut locations = Vec::new();

    // Results is Option<Vec<ReferenceSearchResult>>
    if let Some(search_results) = results {
        for search_result in search_results {
            // Add the declaration if present
            // Declaration.nav is NavigationTarget (not Option)
            if let Some(decl) = &search_result.declaration {
                locations.push(nav_target_to_location(vfs, &analysis, &decl.nav)?);
            }

            // Add all references
            // references is IntMap<FileId, Vec<(TextRange, ReferenceCategory)>>
            for (ref_file_id, refs) in &search_result.references {
                // Convert ide_db::FileId to vfs FileId
                let ref_vfs_file_id = ra_ap_vfs::FileId::from_raw(ref_file_id.index());
                let ref_vfs_path = vfs.file_path(ref_vfs_file_id);
                let ref_file_path: PathBuf = ref_vfs_path.as_path()
                    .ok_or_else(|| anyhow::anyhow!("Not a real path"))?
                    .to_path_buf()
                    .into();

                let ref_line_index = analysis.file_line_index(*ref_file_id)?;

                for (range, _category) in refs {
                    let line_col = ref_line_index.line_col(range.start());
                    locations.push(Location {
                        file_path: ref_file_path.clone(),
                        line: line_col.line + 1,
                        column: line_col.col + 1,
                        name: "reference".to_string(),
                    });
                }
            }
        }
    }

    Ok(locations)
}
