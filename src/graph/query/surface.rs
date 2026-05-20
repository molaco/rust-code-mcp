//! Query methods on `OpenedSnapshot` — public-surface family.
//!
//! Covers public-surface queries: `enum_variants`, `item_attributes`,
//! `items_with_attribute`, `pub_use_pub_type_audit`, `re_export_chain`,
//! `dead_pub_in_crate`, `dead_pub_report`. Moved here from `graph::queries`
//! in PR 10.

use std::collections::HashSet;

use anyhow::Result;

use super::super::ids::NodeId;
use super::super::model::{
    Binding, BindingKind, BindingVisibility, ItemKind, Node, NodeKind, Usage,
};
use super::super::queries::MAX_REEXPORT_HOPS;
use super::super::snapshot::OpenedSnapshot;
use super::model::{
    CrateDeadPub, DeadPubFinding, ItemWithAttribute, PubTypeAliasMasqueradingAsReexport,
    ReExportChain, ReExportLink,
};

impl OpenedSnapshot {
    /// v7: enumerate the variants of an enum. `enum_id` must be the NodeId of
    /// an `ItemKind::Enum` Item; non-enum inputs return an empty Vec rather
    /// than erroring (so callers can probe arbitrary Items without
    /// pre-validating). Walks `children_by_parent` and filters to
    /// `item_kind == ItemKind::EnumVariant` — explicit filter even though
    /// today an enum's only children are its variants, so future model
    /// extensions don't silently leak through.
    pub fn enum_variants(&self, enum_id: NodeId) -> Result<Vec<Node>> {
        let rtxn = self.env.read_txn()?;
        let mut child_ids: Vec<NodeId> = Vec::new();
        if let Some(iter) = self
            .dbs
            .children_by_parent
            .get_duplicates(&rtxn, enum_id.as_bytes())?
        {
            for entry in iter {
                let (_k, child_bytes) = entry?;
                let mut id = [0u8; 32];
                id.copy_from_slice(child_bytes);
                child_ids.push(NodeId(id));
            }
        }
        let mut out = Vec::with_capacity(child_ids.len());
        for child_id in child_ids {
            let Some(node) = self.dbs.nodes_by_id.get(&rtxn, child_id.as_bytes())? else {
                continue;
            };
            if node.item_kind == Some(ItemKind::EnumVariant) {
                out.push(node);
            }
        }
        // Sort by source position so variants come back in declaration order
        // rather than alphabetically. Nodes without spans (rare; only ones
        // missing nav-target metadata) sort to the end with empty file path
        // and (0, 0) span to keep ordering deterministic.
        out.sort_by(|a, b| {
            let a_key = (
                a.file.as_deref().unwrap_or(""),
                a.span.map(|(s, _)| s).unwrap_or(0),
            );
            let b_key = (
                b.file.as_deref().unwrap_or(""),
                b.span.map(|(s, _)| s).unwrap_or(0),
            );
            a_key.cmp(&b_key)
        });
        Ok(out)
    }

    /// v8: return the outer attributes and doc-comment lines (one entry per
    /// line) recorded for the Item at `target`. Empty Vec when the target
    /// has no attributes, isn't an Item, or doesn't exist. Order matches
    /// source order.
    pub fn item_attributes(&self, target: NodeId) -> Result<Vec<String>> {
        let rtxn = self.env.read_txn()?;
        let Some(node) = self.dbs.nodes_by_id.get(&rtxn, target.as_bytes())? else {
            return Ok(Vec::new());
        };
        Ok(node.attributes)
    }

    /// v8: every Item in `crate_id` whose attribute list has at least one
    /// entry that matches `attr_pattern`. Wrapped patterns match
    /// case-sensitively at the **start** of the raw attribute string (e.g.
    /// attr `"#[must_use]"` matches pattern `"#[must_use]"` and pattern
    /// `"#[must_use"`), bare patterns such as `derive` or `must_use` match
    /// the attribute path, OR a pattern can match the start of the **body**
    /// of a `///` doc comment (the body is whatever follows the `/// ` prefix). This
    /// avoids false positives where the pattern text appears in the middle
    /// of an unrelated attribute — e.g. searching `#[must_use]` no longer
    /// matches `#[tool(description = "...#[must_use]...")]` whose body just
    /// happens to mention the pattern. Empty patterns match nothing.
    /// Returns enriched rows with file + span so callers can navigate.
    /// Sorted by qualified name.
    pub fn items_with_attribute(
        &self,
        crate_id: NodeId,
        attr_pattern: &str,
    ) -> Result<Vec<ItemWithAttribute>> {
        let rtxn = self.env.read_txn()?;
        let mut out: Vec<ItemWithAttribute> = Vec::new();
        // Empty pattern: match nothing. Substring containment of "" is
        // trivially true on every string, which would flood callers with
        // every attribute-bearing item — almost certainly not what they
        // wanted. The previous behavior here was the same trivial-true
        // bug; switch to "match nothing" as the safer default.
        if attr_pattern.is_empty() {
            return Ok(out);
        }
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            if node.kind != NodeKind::Item {
                continue;
            }
            if node.crate_id != Some(crate_id) {
                continue;
            }
            let Some((matched, location)) = node
                .attributes
                .iter()
                .find_map(|a| match_attribute(a, attr_pattern).map(|loc| (a.clone(), loc)))
            else {
                continue;
            };
            let mut id = [0u8; 32];
            id.copy_from_slice(key);
            out.push(ItemWithAttribute {
                target: NodeId(id),
                qualified_name: node.qualified_name,
                item_kind: node.item_kind,
                matched_attribute: matched,
                match_location: location.to_string(),
                file: node.file,
                span: node.span,
            });
        }
        out.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
        Ok(out)
    }

    /// Phase 4a (heuristic): every `Item.TypeAlias` in `crate_id` whose
    /// owning module also carries a `pub use ... as <alias_name>` (or
    /// `pub use ::<alias_name>`) binding. Such an alias is a candidate
    /// for being a re-export disguised as a `pub type` declaration.
    ///
    /// Limitation: the model does not record what an alias's RHS resolves
    /// to, so this query cannot confirm the `pub use` and the `pub type`
    /// point at the same target. Treat results as candidates and verify
    /// with `find_definition` / source review before acting.
    pub fn pub_use_pub_type_audit(
        &self,
        crate_id: NodeId,
    ) -> Result<Vec<PubTypeAliasMasqueradingAsReexport>> {
        let rtxn = self.env.read_txn()?;
        // Collect all type-aliases in the crate first so the iterator
        // borrow on rtxn is released before we walk per-alias bindings.
        let mut aliases: Vec<(NodeId, Node)> = Vec::new();
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            if node.kind != NodeKind::Item {
                continue;
            }
            if node.crate_id != Some(crate_id) {
                continue;
            }
            if node.item_kind != Some(ItemKind::TypeAlias) {
                continue;
            }
            let mut id = [0u8; 32];
            id.copy_from_slice(key);
            aliases.push((NodeId(id), node));
        }

        let mut out: Vec<PubTypeAliasMasqueradingAsReexport> = Vec::new();
        for (alias_id, alias) in aliases {
            let Some(owner) = alias.parent_id else {
                continue;
            };
            for entry in self.bindings_for_from_module(&rtxn, owner)? {
                let binding = entry?;
                if !binding.is_explicit_pub_use {
                    continue;
                }
                if binding.visible_name != alias.display_name {
                    continue;
                }
                if binding.target == alias_id {
                    continue; // a binding to the alias itself isn't suspicious
                }
                out.push(PubTypeAliasMasqueradingAsReexport {
                    alias_qualified_name: alias.qualified_name.clone(),
                    alias_node_id: alias_id,
                    file: alias.file.clone(),
                    span: alias.span,
                    suspicious_pub_use_target_node_id: binding.target,
                    suspicious_pub_use_visible_name: binding.visible_name,
                });
            }
        }
        out.sort_by(|a, b| a.alias_qualified_name.cmp(&b.alias_qualified_name));
        Ok(out)
    }

    /// Phase 4b: walk every `pub use` re-export of `target` (and every
    /// re-export of those re-exports) up to `MAX_REEXPORT_HOPS` hops.
    /// Returns one `ReExportLink` per visited binding, breadth-first.
    /// Cycle detection keys on `(from_module, visible_name)` pairs so a
    /// module re-exporting two distinct items under the same name still
    /// surfaces both, while a self-referential cycle terminates.
    pub fn re_export_chain(&self, target: NodeId) -> Result<ReExportChain> {
        let rtxn = self.env.read_txn()?;
        let canonical_node = self
            .dbs
            .nodes_by_id
            .get(&rtxn, target.as_bytes())?
            .map(|n| n.qualified_name)
            .unwrap_or_default();

        let mut links: Vec<ReExportLink> = Vec::new();
        let mut visited: HashSet<(NodeId, String)> = HashSet::new();
        // Frontier: (target_to_search_for, depth_to_assign)
        let mut frontier: Vec<(NodeId, u8)> = vec![(target, 1)];
        while let Some((current_target, depth)) = frontier.pop() {
            if depth as usize > MAX_REEXPORT_HOPS {
                continue;
            }
            // Collect bindings first to release the iterator borrow
            // before per-binding `nodes_by_id.get` lookups.
            let mut bindings_for_current: Vec<Binding> = Vec::new();
            for entry in self.bindings_for_target(&rtxn, current_target)? {
                bindings_for_current.push(entry?);
            }
            for binding in bindings_for_current {
                if !binding.is_explicit_pub_use {
                    continue;
                }
                let key = (binding.from_module, binding.visible_name.clone());
                if !visited.insert(key) {
                    continue;
                }
                let from_module_qualified = self
                    .dbs
                    .nodes_by_id
                    .get(&rtxn, binding.from_module.as_bytes())?
                    .map(|n| n.qualified_name)
                    .unwrap_or_default();
                links.push(ReExportLink {
                    from_module: binding.from_module,
                    from_module_qualified_name: from_module_qualified,
                    visible_name: binding.visible_name.clone(),
                    depth,
                });
                // Recurse: the re-exporting module is itself a "target"
                // for downstream re-exports. Bindings whose target is the
                // module ID don't generally exist, so this naturally
                // terminates when no further hops are found.
                if (depth as usize) < MAX_REEXPORT_HOPS {
                    frontier.push((binding.from_module, depth + 1));
                }
            }
        }
        // Stable order: by depth, then module qualified name, then visible name.
        links.sort_by(|a, b| {
            a.depth
                .cmp(&b.depth)
                .then_with(|| a.from_module_qualified_name.cmp(&b.from_module_qualified_name))
                .then_with(|| a.visible_name.cmp(&b.visible_name))
        });
        Ok(ReExportChain {
            canonical: target,
            canonical_qualified_name: canonical_node,
            links,
        })
    }

    /// Items in `crate_id` declared `pub` whose only consumers — both as imports
    /// and as references — live inside the same crate. Such items are candidates
    /// for downgrading to `pub(crate)`.
    ///
    /// Skipped (already minimal):
    ///   * `Private` items.
    ///   * `pub(crate)` items targeting their own crate.
    ///   * `pub(in path)` items — the path is always an ancestor module within
    ///     the same crate, so visibility is already strictly narrower than
    ///     `pub(crate)`.
    ///
    /// Known false positive: an item referenced *only* through a public
    /// function/type signature (never named directly in caller code) won't show
    /// up in `usages_by_target`, so we may flag it as dead-pub even when its
    /// `pub` is load-bearing for the signature. Acceptable for v1 — caller
    /// should treat findings as candidates, not certainties.
    pub fn dead_pub_in_crate(&self, crate_id: NodeId) -> Result<Vec<DeadPubFinding>> {
        let rtxn = self.env.read_txn()?;

        let mut candidates: Vec<(NodeId, Node)> = Vec::new();
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            if node.kind == NodeKind::Item && node.crate_id == Some(crate_id) {
                let mut id = [0u8; 32];
                id.copy_from_slice(key);
                candidates.push((NodeId(id), node));
            }
        }

        let mut out = Vec::new();
        for (item_id, item) in candidates {
            // Collect bindings before doing follow-up `nodes_by_id.get` lookups
            // so the iterator's borrow on `rtxn` is dropped first.
            let mut bindings_for_item: Vec<Binding> = Vec::new();
            for entry in self.bindings_for_target(&rtxn, item_id)? {
                bindings_for_item.push(entry?);
            }

            // Find the Declared binding (visibility lives there). Items appear
            // in Type and Value namespaces for unit/tuple structs, but the
            // post-extraction dedup keeps just one Declared row.
            let Some(declared) = bindings_for_item
                .iter()
                .find(|b| b.kind == BindingKind::Declared)
                .cloned()
            else {
                continue;
            };

            // Visibility filter — only `Public` items are candidates.
            match declared.visibility {
                BindingVisibility::Public => {}
                BindingVisibility::Private
                | BindingVisibility::Crate(_)
                | BindingVisibility::RestrictedTo(_) => continue,
            }

            // External importer check (any non-Declared binding from another crate).
            let mut has_external_importer = false;
            for binding in &bindings_for_item {
                if binding.kind == BindingKind::Declared {
                    continue;
                }
                let Some(from_node) = self
                    .dbs
                    .nodes_by_id
                    .get(&rtxn, binding.from_module.as_bytes())?
                else {
                    continue;
                };
                if from_node.crate_id != Some(crate_id) {
                    has_external_importer = true;
                    break;
                }
            }
            if has_external_importer {
                continue;
            }

            // External user check (any usage whose consumer module is in
            // another crate). Collect first, then resolve.
            let mut usages_for_item: Vec<Usage> = Vec::new();
            for entry in self.usages_for_target(&rtxn, item_id)? {
                usages_for_item.push(entry?);
            }
            let mut has_external_user = false;
            for usage in &usages_for_item {
                let Some(consumer_node) = self
                    .dbs
                    .nodes_by_id
                    .get(&rtxn, usage.consumer_module.as_bytes())?
                else {
                    continue;
                };
                if consumer_node.crate_id != Some(crate_id) {
                    has_external_user = true;
                    break;
                }
            }
            if has_external_user {
                continue;
            }

            let Some(item_kind) = item.item_kind else {
                continue;
            };
            out.push(DeadPubFinding {
                target: item_id,
                qualified_name: item.qualified_name.clone(),
                item_kind,
                declared_visibility: declared.visibility,
            });
        }
        // Deterministic order — easier to diff across runs.
        out.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
        Ok(out)
    }

    /// Workspace-wide `dead_pub` aggregate: one entry per local crate, with
    /// findings sorted by qualified name. Crates are returned sorted by name.
    pub fn dead_pub_report(&self) -> Result<Vec<CrateDeadPub>> {
        // Gather crate ids first so the per-crate query iterators don't
        // overlap with the outer scan over nodes_by_id.
        let mut crates: Vec<(NodeId, String)> = Vec::new();
        {
            let rtxn = self.env.read_txn()?;
            for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
                let (key, node) = entry?;
                if node.kind == NodeKind::Crate {
                    let mut id = [0u8; 32];
                    id.copy_from_slice(key);
                    crates.push((NodeId(id), node.qualified_name));
                }
            }
        }
        crates.sort_by(|a, b| a.1.cmp(&b.1));

        let mut report = Vec::with_capacity(crates.len());
        for (crate_id, crate_qualified_name) in crates {
            let findings = self.dead_pub_in_crate(crate_id)?;
            report.push(CrateDeadPub {
                crate_id,
                crate_qualified_name,
                findings,
            });
        }
        Ok(report)
    }
}

/// Anchored attribute match used by `items_with_attribute`.
///
/// Returns `Some("attr")` when `pat` is a prefix of the raw attribute
/// string or matches the attribute path (`derive`, `must_use`, `cfg`, ...),
/// `Some("doc")` when the attribute is a `///` doc-comment (`"/// body"`)
/// and `pat` is a prefix of the body, otherwise `None`.
///
/// The intent is that searching for `#[must_use]` matches an attribute
/// stored as `"#[must_use]"` or `"#[must_use = \"...\"]"`, but does NOT
/// match unrelated attributes whose body merely contains the literal text
/// `#[must_use]` (e.g. `#[tool(description = "...#[must_use]...")]`).
///
/// Doc-comment lines are stored verbatim as `"/// body"` (the lexer
/// preserves the leading space). Stripping the `/// ` prefix lets a
/// caller search for `SAFETY` and match a doc line `"/// SAFETY: ..."`,
/// while still matching the full-string form `/// SAFETY` against the
/// raw attribute prefix.
///
/// Empty pattern returns `None` (the caller short-circuits empty
/// patterns to "match nothing").
fn match_attribute(attr: &str, pat: &str) -> Option<&'static str> {
    if pat.is_empty() {
        return None;
    }
    if attr.starts_with(pat) {
        return Some("attr");
    }
    if attr_matches_path_or_body(attr, pat) {
        return Some("attr");
    }
    // Doc lines in our model are always stored as `"/// body"`.
    if let Some(body) = attr.strip_prefix("/// ") {
        if body.starts_with(pat) {
            return Some("doc");
        }
    }
    None
}

fn attr_matches_path_or_body(attr: &str, pat: &str) -> bool {
    let Some(body) = attr.strip_prefix("#[") else {
        return false;
    };
    let normalized_pat = normalize_attr_pattern(pat);
    if normalized_pat.is_empty() {
        return false;
    }
    if attr_pattern_is_path_only(normalized_pat) {
        let path = attr_path(body);
        return path == normalized_pat
            || path
                .rsplit("::")
                .next()
                .map(|last| last == normalized_pat)
                .unwrap_or(false);
    }
    body.starts_with(normalized_pat)
}

fn normalize_attr_pattern(pat: &str) -> &str {
    let pat = pat
        .strip_prefix("#[")
        .unwrap_or(pat)
        .strip_prefix('!')
        .unwrap_or_else(|| pat.strip_prefix("#![").unwrap_or(pat));
    pat.strip_suffix(']').unwrap_or(pat)
}

fn attr_pattern_is_path_only(pat: &str) -> bool {
    !pat.is_empty()
        && pat
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
}

fn attr_path(body: &str) -> &str {
    let end = body
        .char_indices()
        .find_map(|(idx, c)| {
            if c.is_ascii_alphanumeric() || c == '_' || c == ':' {
                None
            } else {
                Some(idx)
            }
        })
        .unwrap_or(body.len());
    &body[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_attribute_accepts_bare_attribute_paths() {
        assert_eq!(match_attribute("#[derive(Debug)]", "derive"), Some("attr"));
        assert_eq!(match_attribute("#[derive(Debug)]", "#[derive("), Some("attr"));
        assert_eq!(match_attribute("#[must_use]", "must_use"), Some("attr"));
        assert_eq!(match_attribute("#[cfg(test)]", "cfg"), Some("attr"));
        assert_eq!(
            match_attribute("#[tool(description = \"mentions #[must_use]\")]", "must_use"),
            None
        );
    }
}
