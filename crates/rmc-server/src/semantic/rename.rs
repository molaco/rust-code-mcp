//! Symbol renaming via rust-analyzer

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ra_ap_ide::{
    AnalysisHost, FilePosition, FileSystemEdit, Query, RenameConfig, SourceChange,
};
use ra_ap_vfs::Vfs;

use super::position;

/// A single text edit to apply to a file
#[derive(Debug, Clone)]
pub(crate) struct RenameEdit {
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
pub(crate) struct RenameFileMove {
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
pub(crate) struct RenamePreview {
    pub edits: Vec<RenameEdit>,
    pub file_moves: Vec<RenameFileMove>,
}

/// Rename the symbol at the given symbol-name. Returns a preview without touching disk.
///
/// Resolves the symbol by name first; fails if multiple symbols match (ambiguous rename
/// is dangerous). Use rename_by_position to disambiguate.
pub(crate) fn rename_by_name(
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
        [] => {
            let fuzzy: Vec<_> = symbols.iter().collect();
            let locs = format_nav_candidates(vfs, &analysis, &fuzzy);
            anyhow::bail!(
                "No exact match for '{}'. Found {} fuzzy candidates. rename_symbol matches the leaf symbol name; use file_path, line, and column to disambiguate a candidate.\n{}",
                symbol_name,
                symbols.len(),
                locs
            )
        }
        [single] => *single,
        multiple => {
            let locs = format_nav_candidates(vfs, &analysis, multiple);
            anyhow::bail!(
                "Ambiguous symbol '{}': {} exact matches.\n{}",
                symbol_name,
                multiple.len(),
                locs
            )
        }
    };

    let offset = target.focus_range.unwrap_or(target.full_range).start();
    let position = FilePosition {
        file_id: target.file_id,
        offset,
    };

    rename_at_file_position(vfs, &analysis, position, new_name)
}

fn nav_position(
    analysis: &ra_ap_ide::Analysis,
    nav: &ra_ap_ide::NavigationTarget,
) -> Result<(u32, u32)> {
    let line_index = analysis.file_line_index(nav.file_id)?;
    let offset = nav.focus_range.unwrap_or(nav.full_range).start();
    let line_col = line_index.line_col(offset);

    Ok((line_col.line + 1, line_col.col + 1))
}

fn format_nav_candidates(
    vfs: &Vfs,
    analysis: &ra_ap_ide::Analysis,
    navs: &[&ra_ap_ide::NavigationTarget],
) -> String {
    navs.iter()
        .map(|nav| format_nav_candidate(vfs, analysis, nav))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_nav_candidate(
    vfs: &Vfs,
    analysis: &ra_ap_ide::Analysis,
    nav: &ra_ap_ide::NavigationTarget,
) -> String {
    let path = vfs
        .file_path(nav.file_id)
        .as_path()
        .map(|p| p.to_string())
        .unwrap_or_else(|| "<virtual>".to_string());
    let (line, column) = nav_position(analysis, nav)
        .map(|(line, column)| (line.to_string(), column.to_string()))
        .unwrap_or_else(|_| ("?".to_string(), "?".to_string()));

    format!(
        "  - {}:{}:{} ({}) - rerun with file_path=\"{}\", line={}, column={}",
        path, line, column, nav.name, path, line, column
    )
}

/// Rename the symbol at a concrete file position. Returns a preview without touching disk.
pub(crate) fn rename_by_position(
    host: &AnalysisHost,
    vfs: &Vfs,
    file_path: &Path,
    line: u32,
    column: u32,
    expected_symbol_name: &str,
    new_name: &str,
) -> Result<RenamePreview> {
    let analysis = host.analysis();
    let position = position::file_position(&analysis, vfs, file_path, line, column)?;

    verify_expected_symbol_at_position(&analysis, position, expected_symbol_name)
        .with_context(|| {
            format!(
                "Expected rename position {}:{}:{} to be on symbol '{}'",
                file_path.display(),
                line,
                column,
                expected_symbol_name
            )
        })?;

    rename_at_file_position(vfs, &analysis, position, new_name)
}

fn rename_at_file_position(
    vfs: &Vfs,
    analysis: &ra_ap_ide::Analysis,
    position: FilePosition,
    new_name: &str,
) -> Result<RenamePreview> {
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

    source_change_to_preview(vfs, analysis, source_change)
}

fn verify_expected_symbol_at_position(
    analysis: &ra_ap_ide::Analysis,
    position: FilePosition,
    expected_symbol_name: &str,
) -> Result<()> {
    let expected = expected_symbol_name
        .rsplit("::")
        .next()
        .unwrap_or(expected_symbol_name)
        .trim();

    if expected.is_empty() {
        anyhow::bail!("symbol_name must not be empty");
    }

    let text = analysis
        .file_text(position.file_id)
        .context("Failed to read file text for rename position")?;
    let offset = usize::from(position.offset);
    let token = identifier_at_offset(&text, offset)
        .ok_or_else(|| anyhow::anyhow!("position is not on an identifier token"))?;

    if token != expected {
        anyhow::bail!("position is on '{}', not '{}'", token, expected);
    }

    Ok(())
}

fn identifier_at_offset(text: &str, offset: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    let probe = if offset < bytes.len() {
        offset
    } else {
        offset.checked_sub(1)?
    };

    if !is_rust_ident_byte(bytes.get(probe).copied()?) {
        return None;
    }

    let mut start = probe;
    while start > 0 && is_rust_ident_byte(bytes[start - 1]) {
        start -= 1;
    }

    let mut end = probe + 1;
    while end < bytes.len() && is_rust_ident_byte(bytes[end]) {
        end += 1;
    }

    text.get(start..end)
}

fn is_rust_ident_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
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
