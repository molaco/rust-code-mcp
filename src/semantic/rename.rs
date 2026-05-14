//! Symbol renaming via rust-analyzer

use std::path::PathBuf;

use anyhow::{Context, Result};
use ra_ap_ide::{
    AnalysisHost, FilePosition, FileSystemEdit, Query, RenameConfig, SourceChange,
};
use ra_ap_vfs::Vfs;

/// A single text edit to apply to a file
#[derive(Debug, Clone)]
pub struct RenameEdit {
    pub file_path: PathBuf,
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub new_text: String,
}

impl std::fmt::Display for RenameEdit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}:{}-{}:{} → {:?}",
            self.file_path.display(),
            self.start_line,
            self.start_column,
            self.end_line,
            self.end_column,
            self.new_text,
        )
    }
}

/// A file move/create as part of a rename (e.g. renaming a module renames a file)
#[derive(Debug, Clone)]
pub struct RenameFileMove {
    pub from: PathBuf,
    pub to_anchor: PathBuf,
    pub to_path: String,
}

impl std::fmt::Display for RenameFileMove {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "move: {} → (anchor: {}) {}",
            self.from.display(),
            self.to_anchor.display(),
            self.to_path,
        )
    }
}

/// Preview of a rename operation
#[derive(Debug, Clone, Default)]
pub struct RenamePreview {
    pub edits: Vec<RenameEdit>,
    pub file_moves: Vec<RenameFileMove>,
}

/// Rename the symbol at the given symbol-name. Returns a preview without touching disk.
///
/// Resolves the symbol by name first; fails if multiple symbols match (ambiguous rename
/// is dangerous). Use a fully-qualified name fragment to disambiguate.
pub fn rename_by_name(
    host: &AnalysisHost,
    vfs: &Vfs,
    symbol_name: &str,
    new_name: &str,
) -> Result<RenamePreview> {
    let analysis = host.analysis();

    let query = Query::new(symbol_name.to_string());
    let symbols = analysis
        .symbol_search(query, 50)
        .context("symbol_search query failed")?;

    if symbols.is_empty() {
        anyhow::bail!("No symbol found matching '{}'", symbol_name);
    }

    // Filter to exact name matches to avoid renaming a substring match
    let exact: Vec<_> = symbols
        .iter()
        .filter(|s| s.name.as_str() == symbol_name)
        .collect();

    let target = match exact.as_slice() {
        [] => anyhow::bail!(
            "No exact match for '{}'. Found {} fuzzy candidates.",
            symbol_name,
            symbols.len()
        ),
        [single] => *single,
        multiple => {
            let locs: Vec<String> = multiple
                .iter()
                .map(|nav| {
                    let path = vfs
                        .file_path(nav.file_id)
                        .as_path()
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "<virtual>".to_string());
                    format!("  - {} ({})", path, nav.name)
                })
                .collect();
            anyhow::bail!(
                "Ambiguous symbol '{}': {} exact matches.\n{}",
                symbol_name,
                multiple.len(),
                locs.join("\n")
            )
        }
    };

    let offset = target.focus_range.unwrap_or(target.full_range).start();
    let position = FilePosition {
        file_id: target.file_id,
        offset,
    };

    let config = RenameConfig {
        prefer_no_std: false,
        prefer_prelude: true,
        prefer_absolute: false,
        show_conflicts: true,
    };

    let source_change: SourceChange = analysis
        .rename(position, new_name, &config)
        .context("rename query cancelled")?
        .map_err(|e| anyhow::anyhow!("rust-analyzer rename refused: {}", e))?;

    source_change_to_preview(vfs, &analysis, source_change)
}

fn source_change_to_preview(
    vfs: &Vfs,
    analysis: &ra_ap_ide::Analysis,
    change: SourceChange,
) -> Result<RenamePreview> {
    let mut preview = RenamePreview::default();

    for (file_id, (text_edit, _snippet)) in &change.source_file_edits {
        let vfs_path = vfs.file_path(*file_id);
        let file_path: PathBuf = vfs_path
            .as_path()
            .ok_or_else(|| anyhow::anyhow!("Edit refers to non-real path"))?
            .to_path_buf()
            .into();

        let line_index = analysis
            .file_line_index(*file_id)
            .context("Failed to get line index for edit")?;

        for indel in text_edit.iter() {
            let start = line_index.line_col(indel.delete.start());
            let end = line_index.line_col(indel.delete.end());
            preview.edits.push(RenameEdit {
                file_path: file_path.clone(),
                start_line: start.line + 1,
                start_column: start.col + 1,
                end_line: end.line + 1,
                end_column: end.col + 1,
                new_text: indel.insert.clone(),
            });
        }
    }

    for fs_edit in &change.file_system_edits {
        match fs_edit {
            FileSystemEdit::CreateFile { dst, .. } => {
                let anchor_path = vfs
                    .file_path(dst.anchor)
                    .as_path()
                    .map(|p| p.to_path_buf().into())
                    .unwrap_or_else(PathBuf::new);
                preview.file_moves.push(RenameFileMove {
                    from: PathBuf::new(),
                    to_anchor: anchor_path,
                    to_path: dst.path.clone(),
                });
            }
            FileSystemEdit::MoveFile { src, dst } => {
                let src_path = vfs
                    .file_path(*src)
                    .as_path()
                    .map(|p| p.to_path_buf().into())
                    .unwrap_or_else(PathBuf::new);
                let anchor_path = vfs
                    .file_path(dst.anchor)
                    .as_path()
                    .map(|p| p.to_path_buf().into())
                    .unwrap_or_else(PathBuf::new);
                preview.file_moves.push(RenameFileMove {
                    from: src_path,
                    to_anchor: anchor_path,
                    to_path: dst.path.clone(),
                });
            }
            FileSystemEdit::MoveDir { src, src_id: _, dst } => {
                let anchor_path = vfs
                    .file_path(dst.anchor)
                    .as_path()
                    .map(|p| p.to_path_buf().into())
                    .unwrap_or_else(PathBuf::new);
                preview.file_moves.push(RenameFileMove {
                    from: PathBuf::from(src.path.as_str()),
                    to_anchor: anchor_path,
                    to_path: dst.path.clone(),
                });
            }
        }
    }

    preview.edits.sort_by(|a, b| {
        a.file_path
            .cmp(&b.file_path)
            .then(a.start_line.cmp(&b.start_line))
            .then(a.start_column.cmp(&b.start_column))
    });

    Ok(preview)
}
