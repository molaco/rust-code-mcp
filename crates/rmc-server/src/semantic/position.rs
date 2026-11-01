//! Position and coordinate utilities

use anyhow::{Context, Result};
use ra_ap_ide::{
    Analysis, AnalysisHost, FilePosition, LineCol, NavigationTarget, Query, SymbolKind, TextSize,
};
use ra_ap_vfs::{Vfs, VfsPath};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A source code location
#[derive(Debug, Clone)]
pub(crate) struct Location {
    pub file_path: PathBuf,
    pub line: u32,   // 1-based
    pub column: u32, // 1-based
    pub name: String,
    pub exact: bool,
}

impl std::fmt::Display for Location {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}:{} ({}, exact={})",
            self.file_path.display(),
            self.line,
            self.column,
            self.name,
            self.exact
        )
    }
}

/// Convert file path to FileId
fn path_to_file_id(vfs: &Vfs, file_path: &Path) -> Result<ra_ap_vfs::FileId> {
    let abs_path = file_path
        .canonicalize()
        .context("Failed to canonicalize path")?;
    let vfs_path = VfsPath::new_real_path(abs_path.to_string_lossy().to_string());

    vfs.file_id(&vfs_path)
        .map(|(id, _)| id)
        .ok_or_else(|| anyhow::anyhow!("File not found in VFS: {}", file_path.display()))
}

/// Convert line/column to byte offset
fn to_offset(
    analysis: &Analysis,
    file_id: ra_ap_vfs::FileId,
    line: u32,
    column: u32,
) -> Result<TextSize> {
    let line_index = analysis
        .file_line_index(file_id)
        .context("Failed to get line index")?;

    // LineCol is 0-based, input is 1-based
    let line_col = LineCol {
        line: line.saturating_sub(1),
        col: column.saturating_sub(1),
    };

    line_index
        .offset(line_col)
        .ok_or_else(|| anyhow::anyhow!("Invalid position: line {}, col {}", line, column))
}

pub(crate) fn file_position(
    analysis: &Analysis,
    vfs: &Vfs,
    file_path: &Path,
    line: u32,
    column: u32,
) -> Result<FilePosition> {
    let file_id = path_to_file_id(vfs, file_path)?;
    let offset = to_offset(analysis, file_id, line, column)?;

    Ok(FilePosition { file_id, offset })
}

/// Convert NavigationTarget to Location
fn nav_target_to_location(
    vfs: &Vfs,
    analysis: &ra_ap_ide::Analysis,
    target: &NavigationTarget,
) -> Result<Location> {
    let vfs_path = vfs.file_path(target.file_id);
    let file_path: PathBuf = vfs_path
        .as_path()
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
        exact: false,
    })
}

/// Goto definition at position
pub(crate) fn goto_definition(
    host: &AnalysisHost,
    vfs: &Vfs,
    file_path: &Path,
    line: u32,
    column: u32,
) -> Result<Vec<Location>> {
    let analysis = host.analysis();
    let position = file_position(&analysis, vfs, file_path, line, column)?;
    let config = ra_ap_ide::GotoDefinitionConfig {
        ra_fixture: ra_ap_ide_db::ra_fixture::RaFixtureConfig::default(),
    };

    let result = analysis
        .goto_definition(position, &config)
        .context("goto_definition query failed")?;

    match result {
        Some(nav_info) => nav_info
            .info
            .iter()
            .map(|target| nav_target_to_location(vfs, &analysis, target))
            .collect(),
        None => Ok(vec![]),
    }
}

/// The reference-search configuration every lookup in this module shares.
///
/// Both exclusions are off on purpose: a `use` line and a `#[cfg(test)]` call
/// site *are* references, and dropping either makes "who uses this" read lower
/// than the truth — the failure this module is built to avoid.
fn all_refs_config() -> ra_ap_ide::FindAllRefsConfig<'static> {
    ra_ap_ide::FindAllRefsConfig {
        ra_fixture: ra_ap_ide_db::ra_fixture::RaFixtureConfig::default(),
        search_scope: None,
        exclude_imports: false,
        exclude_tests: false,
    }
}

/// What a position resolved to — enough to tell the name that was asked for
/// from something else spelled the same way.
struct ResolvedDecl {
    name: String,
    kind: Option<SymbolKind>,
}

/// Everything one `find_all_refs` call turned up, kept apart by role.
///
/// The halves are separated because the two callers want different ones: "who
/// uses this" wants both, "where is this declared" wants only the declarations
/// — and yet still needs the use sites, to know which text occurrences it no
/// longer has to ask about.
#[derive(Default)]
struct Harvest {
    /// Declaration sites of whatever the position resolved to.
    decls: Vec<Location>,
    /// Use sites of the same.
    refs: Vec<Location>,
}

impl Harvest {
    /// Every position this harvest accounts for, in dedup-key shape.
    fn keys(&self) -> impl Iterator<Item = (PathBuf, u32, u32)> + '_ {
        self.decls.iter().chain(&self.refs).map(location_key)
    }

    fn into_all(self) -> Vec<Location> {
        let mut all = self.decls;
        all.extend(self.refs);
        all
    }
}

/// Run `find_all_refs` at `position` and report what it found.
///
/// `None` means the position pointed at no symbol at all. That distinction is
/// the whole reason this returns an `Option`: `find_all_refs` reports "the
/// cursor is not on a name" and "this name has no references" identically, as
/// an empty result, and a caller that cannot tell them apart reports a missed
/// position as "nothing uses this".
fn harvest_refs_at(
    analysis: &Analysis,
    vfs: &Vfs,
    position: FilePosition,
    exact: bool,
) -> Result<Option<(ResolvedDecl, Harvest)>> {
    let Some(search_results) = analysis
        .find_all_refs(position, &all_refs_config())
        .context("find_all_refs query failed")?
    else {
        return Ok(None);
    };

    let mut resolved = None;
    let mut harvest = Harvest::default();

    for search_result in search_results {
        // Declaration.nav is NavigationTarget (not Option)
        if let Some(decl) = &search_result.declaration {
            resolved.get_or_insert_with(|| ResolvedDecl {
                name: decl.nav.name.to_string(),
                kind: decl.nav.kind,
            });
            let mut decl_location = nav_target_to_location(vfs, analysis, &decl.nav)?;
            decl_location.exact = exact;
            harvest.decls.push(decl_location);
        }

        // references is IntMap<FileId, Vec<(TextRange, ReferenceCategory)>>
        for (ref_file_id, refs) in &search_result.references {
            let ref_file_path = file_path_of(vfs, *ref_file_id)?;
            let ref_line_index = analysis.file_line_index(*ref_file_id)?;

            for (range, _category) in refs {
                let line_col = ref_line_index.line_col(range.start());
                harvest.refs.push(Location {
                    file_path: ref_file_path.clone(),
                    line: line_col.line + 1,
                    column: line_col.col + 1,
                    name: "reference".to_string(),
                    exact,
                });
            }
        }
    }

    Ok(resolved.map(|decl| (decl, harvest)))
}

/// Real filesystem path of a file the analysis knows.
fn file_path_of(vfs: &Vfs, file_id: ra_ap_vfs::FileId) -> Result<PathBuf> {
    Ok(vfs
        .file_path(file_id)
        .as_path()
        .ok_or_else(|| anyhow::anyhow!("Not a real path"))?
        .to_path_buf()
        .into())
}

/// Find all references at position
///
/// A position that points at no symbol is an **error**, not an empty list:
/// those two answers are the same string to a caller, and one of them means
/// "ask again, one character to the right".
pub(crate) fn find_references(
    host: &AnalysisHost,
    vfs: &Vfs,
    file_path: &Path,
    line: u32,
    column: u32,
) -> Result<Vec<Location>> {
    let analysis = host.analysis();
    let position = file_position(&analysis, vfs, file_path, line, column)?;

    let Some((_, harvest)) = harvest_refs_at(&analysis, vfs, position, false)? else {
        anyhow::bail!(
            "No symbol at {}:{}:{} — the position does not point at a name (try a column inside the identifier)",
            file_path.display(),
            line,
            column
        );
    };

    Ok(harvest.into_all())
}

/// Search for symbols by name
pub(crate) fn symbol_search(
    host: &AnalysisHost,
    vfs: &Vfs,
    project_root: &Path,
    symbol_name: &str,
    limit: usize,
) -> Result<Vec<Location>> {
    symbol_search_with_exact(&host.analysis(), vfs, project_root, symbol_name, limit, false)
}

/// Search for symbols by name, optionally retaining only full-name matches.
///
/// # The blind spot this covers
///
/// The symbol index carries module-scope declarations plus impl and trait
/// members — a **field** is not in it. So "where is this field declared"
/// answered *no definition found*, the same words a misspelled name gets, and
/// the same words that read as "there is no such thing". When the index
/// produces no full-name match, the sources are asked directly, exactly as
/// `find_references_by_name` does — same sweep, same guard against a
/// same-spelled local.
///
/// # What else the index does not carry: nothing, measured
///
/// One of every kind a caller might name was put to the index with the sweep
/// switched off (`which_kinds_the_symbol_index_carries`). Absent: a struct
/// field, a union field, a field of a struct-shaped enum variant — all three
/// `SymbolKind::Field`, all three already inside the sweep's accept list.
/// Present, some of them against expectation: enum variants, associated consts
/// and types, `macro_rules!` macros, and items a macro expansion produced.
///
/// So the blind spot is *fields*, and it is closed. A tuple struct's field has
/// no identifier to ask about — it is named `0` — so it is outside the question
/// rather than an answer to it.
pub(crate) fn symbol_search_with_exact(
    analysis: &Analysis,
    vfs: &Vfs,
    project_root: &Path,
    symbol_name: &str,
    limit: usize,
    exact_only: bool,
) -> Result<Vec<Location>> {
    let mut locations = index_matches(&analysis, vfs, symbol_name, limit)?;

    if !locations.iter().any(|location| location.exact) {
        collect_by_token_scan(
            &analysis,
            vfs,
            project_root,
            symbol_name,
            Wants::DeclOnly,
            &mut locations,
        )?;
    }

    let mut ranked = rank_and_filter_exact(locations, exact_only);
    dedup_by_position(&mut ranked);
    // The index was asked for `limit` and the sweep may have added to that;
    // the caller's cap is on the answer, not on either source of it.
    ranked.truncate(limit);
    Ok(ranked)
}

/// What rust-analyzer's symbol index alone answers for a name.
///
/// Split out from the search so the index can be questioned on its own: the
/// sweep runs only when the index produced no exact match, which means the
/// search's answer conflates "the index carries this kind" with "the sweep
/// rescued it". Which kinds the index actually carries is a fact worth being
/// able to measure rather than guess — the guess about enum variants was half
/// wrong (see `symbol_search_with_exact`).
fn index_matches(
    analysis: &Analysis,
    vfs: &Vfs,
    symbol_name: &str,
    limit: usize,
) -> Result<Vec<Location>> {
    let query = Query::new(symbol_name.to_string());
    let results = analysis
        .symbol_search(query, limit)
        .context("symbol_search query failed")?;

    results
        .iter()
        .map(|target| {
            let mut location = nav_target_to_location(vfs, analysis, target)?;
            location.exact = location.name == symbol_name;
            Ok(location)
        })
        .collect()
}

/// The index's answer with the sweep deliberately not run — the mutant that
/// tells which kinds the index carries, kept in the source rather than applied
/// by hand to a working tree each time the question comes up.
#[cfg(test)]
pub(crate) fn index_only_search(
    analysis: &Analysis,
    vfs: &Vfs,
    symbol_name: &str,
    limit: usize,
) -> Result<Vec<Location>> {
    let matches = index_matches(&analysis, vfs, symbol_name, limit)?;
    Ok(rank_and_filter_exact(matches, true))
}

/// Collapse repeats of one position, keeping the first — which, after ranking,
/// is the exact match.
///
/// The sweep can arrive at a single declaration several times over. Its skip
/// list is built from the use sites `find_all_refs` reports, and a `Fast`
/// context has no dependency edges, so those stop at the crate boundary: an
/// occurrence in a consuming crate is not on the skip list, gets its own query,
/// and resolves to the same declaration again. Measured on a 4000-file
/// workspace — one field, the same file:line three times over.
fn dedup_by_position(locations: &mut Vec<Location>) {
    let mut seen = HashSet::new();
    locations.retain(|location| seen.insert(location_key(location)));
}

/// Find all references to symbols matching a name
/// First finds all symbols with that name, then finds references for each
pub(crate) fn find_references_by_name(
    host: &AnalysisHost,
    vfs: &Vfs,
    project_root: &Path,
    symbol_name: &str,
) -> Result<Vec<Location>> {
    find_references_by_name_with_exact(&host.analysis(), vfs, project_root, symbol_name, false)
}

/// Find all references to symbols matching a name, optionally retaining only
/// full-name symbol matches before resolving references.
pub(crate) fn find_references_by_name_with_exact(
    analysis: &Analysis,
    vfs: &Vfs,
    project_root: &Path,
    symbol_name: &str,
    exact_only: bool,
) -> Result<Vec<Location>> {
    // First, find all symbols matching the name
    let query = Query::new(symbol_name.to_string());
    let mut symbols = analysis
        .symbol_search(query, 50)
        .context("symbol_search query failed")?;
    symbols.sort_by_key(|symbol| symbol.name.to_string() != symbol_name);

    let mut all_locations = Vec::new();
    let mut resolved_the_name = false;

    // For each symbol found, find its references
    for symbol in &symbols {
        let symbol_exact = symbol.name.to_string() == symbol_name;
        if exact_only && !symbol_exact {
            continue;
        }
        resolved_the_name |= symbol_exact;

        // Get the position of this symbol definition
        let file_id = symbol.file_id;
        let offset = symbol.focus_range.unwrap_or(symbol.full_range).start();
        let position = FilePosition { file_id, offset };

        if let Some((_, harvest)) = harvest_refs_at(&analysis, vfs, position, symbol_exact)? {
            all_locations.extend(harvest.into_all());
        }
    }

    // The symbol index carries module-scope declarations plus impl and trait
    // members. It does NOT carry fields — a struct's, a union's, or a
    // struct-shaped variant's — so a field name matches nothing above and the
    // answer would be an empty list, which reads exactly like "nobody touches
    // this field". Ask the source instead. (Enum variants were expected to
    // share this fate and measurably do not; see `symbol_search_with_exact`.)
    if !resolved_the_name {
        collect_by_token_scan(
            &analysis,
            vfs,
            project_root,
            symbol_name,
            Wants::DeclAndRefs,
            &mut all_locations,
        )?;
    }

    // Deduplicate locations (same file:line:col)
    all_locations.sort_by(|a, b| {
        (&a.file_path, a.line, a.column, !a.exact).cmp(&(&b.file_path, b.line, b.column, !b.exact))
    });
    all_locations
        .dedup_by(|a, b| a.file_path == b.file_path && a.line == b.line && a.column == b.column);
    all_locations.sort_by_key(|location| !location.exact);

    Ok(all_locations)
}

/// What the caller wants out of each definition the sweep resolves.
///
/// The sweep itself is identical either way — the same query brings back the
/// declaration and its uses together — so the choice is only about what reaches
/// the answer. The uses are kept regardless, because they are what lets the
/// sweep skip the remaining occurrences of the name.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wants {
    /// Declaration and every use site — `find_references`.
    DeclAndRefs,
    /// The declaration alone — `find_definition`.
    DeclOnly,
}

/// Resolve `symbol_name` by finding the identifier in the project's own sources
/// and asking rust-analyzer what sits under it.
///
/// This is the path for names the symbol index does not carry — struct fields
/// and enum variants. It runs second because when the index *does* know a name
/// its answer is authoritative and costs one query instead of a file sweep.
///
/// # Why not every occurrence is queried
///
/// The first occurrence that resolves brings back the entire reference set for
/// that definition, which normally accounts for every other occurrence of the
/// name. Those are then skipped, so a field read twenty-one times costs one
/// query rather than twenty-one. What still costs a query each: an occurrence
/// belonging to a *different* definition (a same-named field on another
/// struct — both are wanted, so both are searched), and an occurrence in a
/// comment or string literal, which resolves to nothing.
///
/// # Scope
///
/// Only `.rs` files under `project_root` are swept. Under a full workspace load
/// the VFS also holds every dependency's sources, and a name that happens to
/// exist in a registry crate is not what the caller asked about.
fn collect_by_token_scan(
    analysis: &Analysis,
    vfs: &Vfs,
    project_root: &Path,
    symbol_name: &str,
    wants: Wants,
    out: &mut Vec<Location>,
) -> Result<()> {
    // Sorted, because the VFS hands files back in load order: two runs that
    // probe different occurrences first would otherwise answer in different
    // order.
    let mut files: Vec<ra_ap_vfs::FileId> = vfs
        .iter()
        .filter(|(_, path)| {
            path.as_path().is_some_and(|path| {
                let path: &Path = path.as_ref();
                path.extension().is_some_and(|ext| ext == "rs") && path.starts_with(project_root)
            })
        })
        .map(|(file_id, _)| file_id)
        .collect();
    files.sort();

    // Positions already accounted for, so a definition reached through its
    // first occurrence is not re-queried through its remaining twenty.
    let mut covered: HashSet<(PathBuf, u32, u32)> = out.iter().map(location_key).collect();

    for file_id in files {
        let text = analysis.file_text(file_id)?;
        let line_index = analysis.file_line_index(file_id)?;
        let file_path = file_path_of(vfs, file_id)?;

        for start in identifier_occurrences(&text, symbol_name) {
            let line_col = line_index.line_col(TextSize::new(start as u32));
            let probe = (file_path.clone(), line_col.line + 1, line_col.col + 1);
            if !covered.insert(probe) {
                continue;
            }

            // One character INTO the identifier, not at its first byte:
            // rust-analyzer answers an empty list for a field probed at its
            // first character while answering in full one character later
            // (measured 2026-08-28). Single-character names have no inside.
            let inside = if symbol_name.len() > 1 {
                start + 1
            } else {
                start
            };
            let position = FilePosition {
                file_id,
                offset: TextSize::new(inside as u32),
            };

            let Some((resolved, harvest)) = harvest_refs_at(analysis, vfs, position, true)? else {
                continue;
            };
            if !is_the_wanted_name(&resolved, symbol_name) {
                // A comment, a string, a local binding spelled the same: the
                // occurrence looked right in text and is not this name.
                continue;
            }

            covered.extend(harvest.keys());
            match wants {
                Wants::DeclAndRefs => out.extend(harvest.into_all()),
                Wants::DeclOnly => out.extend(harvest.decls),
            }
        }
    }

    Ok(())
}

/// Whether a resolved definition is the thing the token sweep went looking for.
///
/// The name has to match, and so does the *kind*: the sweep exists for what
/// rust-analyzer's symbol index does not surface — every named **field**, be it
/// a struct's, a union's or a struct-shaped enum variant's; enum variants
/// themselves are a backstop, the index does carry them — so anything else
/// found under a matching
/// identifier is a different entity that happens to share a spelling. A local
/// `let radius_override = 7;` declares a name equal to the field's, and
/// counting it and its uses inflated a four-read field to six — silently, which
/// is the failure mode this whole path exists to end. As a *definition* the
/// same local is worse still: an answer pointing into an unrelated function
/// body, which reads as fact.
fn is_the_wanted_name(decl: &ResolvedDecl, symbol_name: &str) -> bool {
    decl.name == symbol_name && matches!(decl.kind, Some(SymbolKind::Field | SymbolKind::Variant))
}

/// Dedup key: one source position, however it was reached.
fn location_key(location: &Location) -> (PathBuf, u32, u32) {
    (location.file_path.clone(), location.line, location.column)
}

/// Byte offsets at which `name` appears as a whole identifier.
///
/// The boundary test is on `char`, not on bytes, so that a name butting against
/// a multi-byte character in a comment is not mistaken for a longer identifier.
fn identifier_occurrences(text: &str, name: &str) -> Vec<usize> {
    if name.is_empty() {
        return Vec::new();
    }
    let is_ident_char = |c: char| c.is_alphanumeric() || c == '_';

    text.match_indices(name)
        .filter(|(start, _)| {
            let before_ok = text[..*start]
                .chars()
                .next_back()
                .is_none_or(|c| !is_ident_char(c));
            let after_ok = text[start + name.len()..]
                .chars()
                .next()
                .is_none_or(|c| !is_ident_char(c));
            before_ok && after_ok
        })
        .map(|(start, _)| start)
        .collect()
}

fn rank_and_filter_exact(mut locations: Vec<Location>, exact_only: bool) -> Vec<Location> {
    if exact_only {
        locations.retain(|location| location.exact);
    }
    locations.sort_by_key(|location| !location.exact);
    locations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(name: &str, exact: bool, line: u32) -> Location {
        Location {
            file_path: PathBuf::from(format!("{name}.rs")),
            line,
            column: 1,
            name: name.to_string(),
            exact,
        }
    }

    #[test]
    fn rank_and_filter_exact_keeps_substrings_by_default_but_ranks_exact_first() {
        let locations = vec![
            loc("VectorSearchResult", false, 2),
            loc("SearchResult", true, 1),
        ];
        let ranked = rank_and_filter_exact(locations, false);

        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].name, "SearchResult");
        assert!(ranked[0].exact);
        assert_eq!(ranked[1].name, "VectorSearchResult");
        assert!(!ranked[1].exact);
    }

    /// One declaration reached through several text occurrences is still one
    /// declaration. Without this the sweep answered a single field with three
    /// identical lines on a real workspace.
    #[test]
    fn dedup_by_position_collapses_one_declaration_reached_twice() {
        let mut locations = vec![
            loc("radius_override", true, 280),
            loc("radius_override", true, 280),
            loc("radius_override", true, 60),
        ];
        dedup_by_position(&mut locations);

        assert_eq!(locations.len(), 2, "got {locations:?}");
        assert_eq!(locations[0].line, 280);
        assert_eq!(locations[1].line, 60);
    }

    #[test]
    fn rank_and_filter_exact_can_drop_substring_matches() {
        let locations = vec![
            loc("VectorSearchResult", false, 2),
            loc("SearchResult", true, 1),
        ];
        let ranked = rank_and_filter_exact(locations, true);

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].name, "SearchResult");
        assert!(ranked[0].exact);
    }
}
