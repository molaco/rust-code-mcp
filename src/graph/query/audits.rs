//! Query methods on `OpenedSnapshot` — audits family.
//!
//! Covers audit-style queries: `static_metadata`, `mut_static_audit`,
//! `unsafe_audit`. Also hosts the `classify_metadata` free fn and the
//! `MUT_STATIC_PATTERNS` const used by `mut_static_audit`. Moved here
//! from `graph::queries` in PR 10.

use anyhow::Result;

use super::super::ids::NodeId;
use super::super::model::{ItemKind, Node, NodeKind, StaticMetadata};
use super::super::snapshot::OpenedSnapshot;
use super::model::MutStaticFinding;

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
pub(crate) const MUT_STATIC_PATTERNS: &[(&str, &str)] = &[
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
pub fn classify_metadata(meta: &StaticMetadata) -> Vec<&'static str> {
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
