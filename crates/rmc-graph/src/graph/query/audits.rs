//! Query methods on `OpenedSnapshot` — audits family.
//!
//! Covers audit-style queries: `static_metadata`, `mut_static_audit`,
//! `unsafe_audit`. Also hosts the `classify_metadata` free fn and the
//! `MUT_STATIC_PATTERNS` const used by `mut_static_audit`. Moved here
//! from `graph::queries` in PR 10.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::super::channel_audit;
use super::super::derive_audit;
use super::super::docs_audit;
use super::super::fn_body_audit;
use super::super::ids::NodeId;
use super::super::labels::item_kind_display_label;
use super::super::loader;
use super::super::model::{ItemKind, Node, NodeKind, StaticMetadata};
use super::super::recursion_check;
use super::super::snapshot::OpenedSnapshot;
use super::super::storage::{GraphEnvOptions, GraphPaths};
use super::super::unsafe_audit;
use super::model::{
    ChannelCapacityFinding, DeriveAuditFinding, FnBodyAuditFinding, FnBodyAuditOutput,
    MissingDocsAuditFinding, MutStaticAuditFinding, MutStaticFinding, RecursionCheckOutput,
    RecursionCycle, UnsafeAuditFinding,
};

#[derive(Debug, Clone, Default)]
pub struct RecursionCheckOptions {
    pub crate_name: Option<String>,
    pub max_cycle_length: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct ChannelCapacityAuditOptions {
    pub crate_name: Option<String>,
    pub skip_test_fns: bool,
}

#[derive(Debug, Clone)]
pub struct FnBodyAuditOptions {
    pub crate_name: Option<String>,
    pub patterns: Option<Vec<String>>,
    pub skip_test_fns: bool,
}

#[derive(Debug, Clone)]
pub struct MissingDocsAuditOptions {
    pub crate_name: Option<String>,
    pub kind_filter: Option<HashSet<ItemKind>>,
    pub skip_test_items: bool,
}

#[derive(Debug, Clone)]
pub struct DeriveAuditOptions {
    pub crate_name: Option<String>,
    pub kind_filter: Option<HashSet<ItemKind>>,
    pub required_derives: HashSet<String>,
    pub pub_only: bool,
    pub skip_test_items: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum GraphAuditError {
    #[error("failed to canonicalize {directory}: {source}")]
    InvalidDirectory {
        directory: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("no snapshot at {directory}")]
    MissingSnapshot { directory: PathBuf },
    #[error("no node found for qualified name `{0}`")]
    UnknownCrateFilter(String),
    #[error("`{name}` is a {kind:?}, expected a Crate or its root Module")]
    InvalidCrateFilterKind { name: String, kind: NodeKind },
    #[error("{0}")]
    InvalidPattern(String),
}

pub fn run_unsafe_audit(directory: &Path) -> Result<Vec<UnsafeAuditFinding>> {
    let canonical = canonicalize_directory(directory)?;
    let snap = open_directory_snapshot(&canonical)?;
    let loaded = loader::load(&canonical)?;
    let findings = snap.unsafe_audit(&loaded)?;
    Ok(render_unsafe_findings(findings))
}

pub fn run_mut_static_audit(directory: &Path) -> Result<Vec<MutStaticAuditFinding>> {
    let canonical = canonicalize_directory(directory)?;
    let snap = open_directory_snapshot(&canonical)?;
    let findings = snap.mut_static_audit()?;
    Ok(render_mut_static_findings(findings))
}

pub fn run_recursion_check(
    directory: &Path,
    options: RecursionCheckOptions,
) -> Result<RecursionCheckOutput> {
    let canonical = canonicalize_directory(directory)?;
    let snap = open_directory_snapshot(&canonical)?;
    let crate_id_filter = resolve_crate_filter(&snap, options.crate_name.as_deref())?;
    let max_cycle_length = recursion_check::clamp_cycle_length(options.max_cycle_length);
    let cycles = recursion_check::recursion_check(
        &snap,
        recursion_check::RecursionOpts {
            crate_id_filter,
            max_cycle_length,
        },
    )?;
    let mut rendered = Vec::with_capacity(cycles.len());
    for cycle in cycles {
        let fns = recursion_check::enclosing_fn_qualified_names(&snap, &cycle.fns)?;
        let starting_node_id = cycle
            .fns
            .first()
            .map(|id| id.to_hex())
            .unwrap_or_default();
        rendered.push(RecursionCycle {
            fns,
            cycle_length: cycle.cycle_length,
            direct_recursion: cycle.direct_recursion,
            starting_node_id,
        });
    }
    Ok(RecursionCheckOutput {
        max_cycle_length,
        cycles: rendered,
    })
}

pub fn run_channel_capacity_audit(
    directory: &Path,
    options: ChannelCapacityAuditOptions,
) -> Result<Vec<ChannelCapacityFinding>> {
    let canonical = canonicalize_directory(directory)?;
    let snap = open_directory_snapshot(&canonical)?;
    let crate_id_filter = resolve_crate_filter(&snap, options.crate_name.as_deref())?;
    let loaded = loader::load(&canonical)?;
    let findings = channel_audit::channel_capacity_audit(
        &loaded,
        &snap,
        channel_audit::ChannelAuditOpts {
            crate_id_filter,
            skip_test_fns: options.skip_test_fns,
        },
    )?;
    Ok(render_channel_capacity_findings(findings))
}

pub fn run_fn_body_audit(
    directory: &Path,
    options: FnBodyAuditOptions,
) -> Result<FnBodyAuditOutput> {
    let patterns = fn_body_audit::parse_pattern_filter(options.patterns.as_deref())
        .map_err(GraphAuditError::InvalidPattern)?;
    let patterns_used = sorted_pattern_names(&patterns);

    let canonical = canonicalize_directory(directory)?;
    let snap = open_directory_snapshot(&canonical)?;
    let crate_id_filter = resolve_crate_filter(&snap, options.crate_name.as_deref())?;
    let loaded = loader::load(&canonical)?;
    let findings = fn_body_audit::fn_body_audit(
        &loaded,
        &snap,
        fn_body_audit::FnBodyAuditOpts {
            crate_id_filter,
            patterns,
            skip_test_fns: options.skip_test_fns,
        },
    )?;
    Ok(FnBodyAuditOutput {
        patterns_used,
        findings: render_fn_body_findings(findings),
    })
}

pub fn run_missing_docs_audit(
    snap: &OpenedSnapshot,
    options: MissingDocsAuditOptions,
) -> Result<Vec<MissingDocsAuditFinding>> {
    let crate_id_filter = resolve_crate_filter(snap, options.crate_name.as_deref())?;
    let kind_filter = options
        .kind_filter
        .unwrap_or_else(docs_audit::default_kind_filter);
    let findings = docs_audit::missing_docs_audit(
        snap,
        docs_audit::DocsAuditOpts {
            crate_id_filter,
            kind_filter,
            skip_test_items: options.skip_test_items,
        },
    )?;
    Ok(render_missing_docs_findings(findings))
}

pub fn run_derive_audit(
    snap: &OpenedSnapshot,
    options: DeriveAuditOptions,
) -> Result<Vec<DeriveAuditFinding>> {
    let crate_id_filter = resolve_crate_filter(snap, options.crate_name.as_deref())?;
    let kind_filter = options
        .kind_filter
        .unwrap_or_else(derive_audit::default_kind_filter);
    let findings = derive_audit::derive_audit(
        snap,
        derive_audit::DeriveAuditOpts {
            crate_id_filter,
            kind_filter,
            required_derives: options.required_derives,
            pub_only: options.pub_only,
            skip_test_items: options.skip_test_items,
        },
    )?;
    Ok(render_derive_findings(findings))
}

fn render_unsafe_findings(findings: Vec<unsafe_audit::UnsafeFinding>) -> Vec<UnsafeAuditFinding> {
    findings
        .into_iter()
        .map(|finding| UnsafeAuditFinding {
            file: finding.file,
            span: finding.span,
            line_count: finding.line_count,
            enclosing_function: finding.enclosing_function.map(|id| id.to_hex()),
            enclosing_function_name: finding.enclosing_function_name,
            has_safety_comment: finding.has_safety_comment,
        })
        .collect()
}

fn render_mut_static_findings(findings: Vec<MutStaticFinding>) -> Vec<MutStaticAuditFinding> {
    findings
        .into_iter()
        .map(|finding| MutStaticAuditFinding {
            item: finding.item.to_hex(),
            qualified_name: finding.qualified_name,
            matched_pattern: finding.matched_pattern,
            type_string: finding.type_string,
            file: finding.file,
            span: finding.span,
        })
        .collect()
}

fn render_channel_capacity_findings(
    findings: Vec<channel_audit::ChannelFinding>,
) -> Vec<ChannelCapacityFinding> {
    findings
        .into_iter()
        .map(|finding| ChannelCapacityFinding {
            crate_name: finding.crate_name,
            kind: finding.kind,
            bounded: finding.bounded,
            capacity: finding.capacity,
            file: finding.file,
            span: finding.span,
            enclosing_function: finding.enclosing_function.map(|id| id.to_hex()),
            enclosing_function_name: finding.enclosing_function_name,
        })
        .collect()
}

fn render_fn_body_findings(findings: Vec<fn_body_audit::FnBodyFinding>) -> Vec<FnBodyAuditFinding> {
    findings
        .into_iter()
        .map(|finding| FnBodyAuditFinding {
            target: finding.target.map(|id| id.to_hex()),
            qualified_name: finding.qualified_name,
            pattern: finding.pattern,
            file: finding.file,
            span: finding.span,
            context: finding.context,
        })
        .collect()
}

fn render_missing_docs_findings(
    findings: Vec<docs_audit::MissingDocsFinding>,
) -> Vec<MissingDocsAuditFinding> {
    findings
        .into_iter()
        .map(|finding| MissingDocsAuditFinding {
            target: finding.target.to_hex(),
            qualified_name: finding.qualified_name,
            item_kind: item_kind_display_label(finding.item_kind).to_string(),
            visibility: finding.visibility,
            file: finding.file,
            span: finding.span,
        })
        .collect()
}

fn render_derive_findings(findings: Vec<derive_audit::DeriveFinding>) -> Vec<DeriveAuditFinding> {
    findings
        .into_iter()
        .map(|finding| DeriveAuditFinding {
            target: finding.target.to_hex(),
            qualified_name: finding.qualified_name,
            item_kind: item_kind_display_label(finding.item_kind).to_string(),
            visibility: finding.visibility,
            file: finding.file,
            span: finding.span,
            current_derives: finding.current_derives,
            missing_derives: finding.missing_derives,
        })
        .collect()
}

fn sorted_pattern_names(
    patterns: &std::collections::HashSet<&'static str>,
) -> Vec<String> {
    let mut patterns_used: Vec<String> = patterns.iter().map(|pattern| pattern.to_string()).collect();
    patterns_used.sort();
    patterns_used
}

fn canonicalize_directory(directory: &Path) -> Result<PathBuf> {
    directory
        .canonicalize()
        .map_err(|source| GraphAuditError::InvalidDirectory {
            directory: directory.to_path_buf(),
            source,
        }
        .into())
}

fn open_directory_snapshot(directory: &Path) -> Result<OpenedSnapshot> {
    let paths = GraphPaths::for_workspace(directory);
    match super::super::snapshot::open_current(&paths, GraphEnvOptions::default())? {
        Some(snapshot) => Ok(snapshot),
        None => Err(GraphAuditError::MissingSnapshot {
            directory: directory.to_path_buf(),
        }
        .into()),
    }
}

fn resolve_crate_filter(snap: &OpenedSnapshot, crate_name: Option<&str>) -> Result<Option<NodeId>> {
    let Some(qn) = crate_name else {
        return Ok(None);
    };
    let (id, node) = snap
        .lookup_by_qualified_name(qn)?
        .ok_or_else(|| GraphAuditError::UnknownCrateFilter(qn.to_owned()))?;
    let crate_id = match node.kind {
        NodeKind::Crate => id,
        NodeKind::Module => node
            .crate_id
            .or(node.parent_id)
            .ok_or_else(|| anyhow::anyhow!("`{qn}` resolves to a Module with no crate_id"))?,
        other => {
            return Err(GraphAuditError::InvalidCrateFilterKind {
                name: qn.to_owned(),
                kind: other,
            }
            .into());
        }
    };
    Ok(Some(crate_id))
}

/// Pattern classification table for `mut_static_audit`.
///
/// Each entry is `(label, type_substring)`. The classifier scans the static's
/// `type_string` (HirDisplay output) for a literal substring match. The
/// `static mut` pattern is special-cased on `is_mut` rather than on the type
/// string, but it lives in the same const so the documented pattern list
/// stays in one place.
///
/// Why these four:
/// - `static mut` — no synchronization; UB on data race; hard to reason about.
/// - `LazyLock` — globally-mutable interior state initialized on first access.
/// - `OnceLock` — single-write global; still global mutable identity.
/// - `OnceCell` — interior mutability via lazy init, single-thread-only for
///   the std variant.
///
/// `lazy_static!` is intentionally NOT here — Path B's strength is type-name
/// detection, and the macro expands to a generated wrapper type whose name
/// won't match `LazyLock`. Document this as a limitation.
const MUT_STATIC_PATTERNS: &[(&str, &str)] = &[
    // Handled specially via `is_mut`, but listed for documentation parity.
    ("static mut", ""),
    ("LazyLock", "LazyLock"),
    ("OnceLock", "OnceLock"),
    ("OnceCell", "OnceCell"),
];

/// Classify a `StaticMetadata` against `MUT_STATIC_PATTERNS`. Returns the
/// list of matched pattern labels (empty if none). Public to the graph
/// module for unit testing without a snapshot — the workspace-wide audit
/// uses this internally.
pub(crate) fn classify_metadata(meta: &StaticMetadata) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for &(label, needle) in MUT_STATIC_PATTERNS {
        if label == "static mut" {
            if meta.is_mut {
                out.push(label);
            }
        } else if !needle.is_empty() && meta.type_string.contains(needle) {
            out.push(label);
        }
    }
    out
}

impl OpenedSnapshot {
    /// v10 (Phase 7 Path B): return the recorded `StaticMetadata` for
    /// `target` (a local `static` NodeId), or `None` if no metadata is
    /// present (e.g. the target isn't a `static`, or extraction skipped
    /// it). Single-key LMDB lookup, no scan.
    pub fn static_metadata(&self, target: NodeId) -> Result<Option<StaticMetadata>> {
        let rtxn = self.env.read_txn()?;
        Ok(self
            .dbs
            .static_metadata_by_target
            .get(&rtxn, target.as_bytes())?)
    }

    /// v10 (Phase 7 Path B): workspace-wide audit of every local `static`
    /// item whose `StaticMetadata` matches one of the known global-mutable-
    /// state patterns (`static mut`, `LazyLock<...>`, `OnceLock<...>`,
    /// `OnceCell<...>`). A single static matching multiple patterns
    /// produces multiple findings (e.g. `static mut FOO: LazyLock<...>`
    /// yields two rows).
    ///
    /// Iterates `Item` nodes whose `item_kind == ItemKind::Static`, fetches
    /// each one's `StaticMetadata` via `static_metadata_by_target`, and
    /// classifies via `classify_metadata`. Sorted by
    /// `(qualified_name, matched_pattern)` for determinism.
    ///
    /// Limitation: the `lazy_static!` macro is NOT detected — its expansion
    /// produces a generated wrapper type whose name doesn't contain
    /// `LazyLock`. Use `items_with_attribute` or grep for `lazy_static!`
    /// invocations to cover that case.
    pub fn mut_static_audit(&self) -> Result<Vec<MutStaticFinding>> {
        let rtxn = self.env.read_txn()?;

        // Collect static Item nodes first so the iterator borrow on rtxn
        // is released before per-target metadata lookups.
        let mut statics: Vec<(NodeId, Node)> = Vec::new();
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            if node.kind != NodeKind::Item {
                continue;
            }
            if node.item_kind != Some(ItemKind::Static) {
                continue;
            }
            let mut id = [0u8; 32];
            id.copy_from_slice(key);
            statics.push((NodeId(id), node));
        }

        let mut out: Vec<MutStaticFinding> = Vec::new();
        for (item_id, item) in statics {
            let Some(meta) = self
                .dbs
                .static_metadata_by_target
                .get(&rtxn, item_id.as_bytes())?
            else {
                continue;
            };
            let matched = classify_metadata(&meta);
            for label in matched {
                out.push(MutStaticFinding {
                    item: item_id,
                    qualified_name: item.qualified_name.clone(),
                    matched_pattern: label.to_string(),
                    type_string: meta.type_string.clone(),
                    file: item.file.clone(),
                    span: item.span,
                });
            }
        }
        out.sort_by(|a, b| {
            a.qualified_name
                .cmp(&b.qualified_name)
                .then_with(|| a.matched_pattern.cmp(&b.matched_pattern))
        });
        Ok(out)
    }

    /// Phase 6: query-time audit of `unsafe { ... }` blocks across the
    /// workspace. Live computation (no cache); requires a `LoadedWorkspace`
    /// supplied by the caller. Implementation lives in
    /// `crate::graph::unsafe_audit`.
    pub fn unsafe_audit(
        &self,
        loaded: &super::super::loader::LoadedWorkspace,
    ) -> Result<Vec<super::super::unsafe_audit::UnsafeFinding>> {
        super::super::unsafe_audit::unsafe_audit_impl(loaded, self)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn node_id(byte: u8) -> NodeId {
        NodeId([byte; 32])
    }

    #[test]
    fn audit_dto_rendering_converts_unsafe_ids_to_hex() {
        let enclosing = node_id(7);
        let rows = render_unsafe_findings(vec![unsafe_audit::UnsafeFinding {
            file: "src/lib.rs".to_string(),
            span: (10, 20),
            line_count: 3,
            enclosing_function: Some(enclosing),
            enclosing_function_name: Some("crate::f".to_string()),
            has_safety_comment: true,
        }]);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].file, "src/lib.rs");
        assert_eq!(rows[0].span, (10, 20));
        assert_eq!(rows[0].line_count, 3);
        assert_eq!(rows[0].enclosing_function.as_deref(), Some(enclosing.to_hex().as_str()));
        assert_eq!(rows[0].enclosing_function_name.as_deref(), Some("crate::f"));
        assert!(rows[0].has_safety_comment);
    }

    #[test]
    fn audit_dto_rendering_converts_mut_static_ids_to_hex() {
        let item = node_id(9);
        let rows = render_mut_static_findings(vec![MutStaticFinding {
            item,
            qualified_name: "crate::STATE".to_string(),
            matched_pattern: "OnceLock".to_string(),
            type_string: "OnceLock<String>".to_string(),
            file: Some("src/state.rs".to_string()),
            span: Some((30, 42)),
        }]);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].item, item.to_hex());
        assert_eq!(rows[0].qualified_name, "crate::STATE");
        assert_eq!(rows[0].matched_pattern, "OnceLock");
        assert_eq!(rows[0].type_string, "OnceLock<String>");
        assert_eq!(rows[0].file.as_deref(), Some("src/state.rs"));
        assert_eq!(rows[0].span, Some((30, 42)));
    }

    #[test]
    fn audit_dto_rendering_converts_channel_ids_to_hex() {
        let enclosing = node_id(11);
        let rows = render_channel_capacity_findings(vec![channel_audit::ChannelFinding {
            crate_name: "rmc_server".to_string(),
            kind: "tokio_mpsc".to_string(),
            bounded: true,
            capacity: Some(64),
            file: "src/channel.rs".to_string(),
            span: (50, 70),
            enclosing_function: Some(enclosing),
            enclosing_function_name: Some("crate::spawn".to_string()),
        }]);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].crate_name, "rmc_server");
        assert_eq!(rows[0].kind, "tokio_mpsc");
        assert!(rows[0].bounded);
        assert_eq!(rows[0].capacity, Some(64));
        assert_eq!(rows[0].file, "src/channel.rs");
        assert_eq!(rows[0].span, (50, 70));
        assert_eq!(rows[0].enclosing_function.as_deref(), Some(enclosing.to_hex().as_str()));
        assert_eq!(rows[0].enclosing_function_name.as_deref(), Some("crate::spawn"));
    }

    #[test]
    fn audit_dto_rendering_converts_fn_body_ids_and_sorts_patterns() {
        let target = node_id(13);
        let rows = render_fn_body_findings(vec![fn_body_audit::FnBodyFinding {
            target: Some(target),
            qualified_name: Some("crate::fallible".to_string()),
            pattern: "unwrap".to_string(),
            file: "src/fallible.rs".to_string(),
            span: (80, 95),
            context: "value.unwrap()".to_string(),
        }]);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target.as_deref(), Some(target.to_hex().as_str()));
        assert_eq!(rows[0].qualified_name.as_deref(), Some("crate::fallible"));
        assert_eq!(rows[0].pattern, "unwrap");
        assert_eq!(rows[0].file, "src/fallible.rs");
        assert_eq!(rows[0].span, (80, 95));
        assert_eq!(rows[0].context, "value.unwrap()");

        let patterns = HashSet::from(["unwrap", "panic_macros", "expect"]);
        assert_eq!(
            sorted_pattern_names(&patterns),
            vec!["expect", "panic_macros", "unwrap"]
        );
    }
}
