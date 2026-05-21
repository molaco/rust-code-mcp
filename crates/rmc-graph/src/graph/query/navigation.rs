//! Lower-level `OpenedSnapshot` navigation primitives.
//!
//! Name resolution (`lookup_by_qualified_name`), node-by-id lookups, and
//! reverse-call shortcuts — the substrate every other `query/*` family
//! builds on. Separated from `queries.rs` in PR 11 review fix-up so the
//! legacy module can be a true facade.

use std::collections::HashSet;

use anyhow::Result;
use heed::RoTxn;

use super::super::ids::NodeId;
use super::super::model::{BindingKind, ItemKind, Node, NodeKind};
use super::super::snapshot::OpenedSnapshot;
use super::shared::MAX_REEXPORT_HOPS;

pub(in crate::graph) fn impl_module_item_alias_parts(name: &str) -> Option<(&str, &str, &str)> {
    let (type_prefix, member_name) = name.rsplit_once("::")?;
    let (module_prefix, type_name) = type_prefix.rsplit_once("::")?;
    if module_prefix.is_empty() || type_name.is_empty() || member_name.is_empty() {
        return None;
    }
    Some((module_prefix, type_name, member_name))
}

pub(in crate::graph) fn is_impl_module_item_alias_candidate(
    node: &Node,
    module_crate_id: Option<NodeId>,
    module_file: Option<&str>,
    type_name: &str,
    member_name: &str,
) -> bool {
    if node.kind != NodeKind::Item
        || !matches!(
            node.item_kind,
            Some(
                ItemKind::Method
                    | ItemKind::AssocFunction
                    | ItemKind::AssocConst
                    | ItemKind::AssocType
            )
        )
        || node.display_name != member_name
    {
        return false;
    }
    if let Some(crate_id) = module_crate_id {
        if node.crate_id != Some(crate_id) {
            return false;
        }
    }
    if let Some(file) = module_file {
        if node.file.as_deref() != Some(file) {
            return false;
        }
    }

    let suffix = format!("::{type_name}::{member_name}");
    node.qualified_name.ends_with(&suffix)
}

impl OpenedSnapshot {
    /// Resolve a `::`-qualified name to a `(NodeId, Node)`.
    ///
    /// Two-phase lookup:
    ///   1. Canonical match — scan `nodes_by_id` for a `Node.qualified_name == name`.
    ///      This is the common case (declarations live at their canonical path).
    ///   2. Re-export facade fallback — if Phase 1 misses, treat the name as
    ///      `<prefix>::<leaf>`, recursively resolve `<prefix>` (the prefix may
    ///      itself be a re-export facade), then look for a non-Declared binding
    ///      in that module whose `visible_name == leaf` and follow its `target`.
    ///
    /// Recursion is bounded by `MAX_REEXPORT_HOPS` so the function terminates
    /// even in the presence of a binding cycle. The resolved target is returned
    /// as-is, including `ExternalSymbol` stubs — callers that want to walk past
    /// the workspace boundary need to handle that themselves.
    pub fn lookup_by_qualified_name(&self, name: &str) -> Result<Option<(NodeId, Node)>> {
        self.lookup_by_qualified_name_inner(name, MAX_REEXPORT_HOPS)
    }

    fn lookup_by_qualified_name_inner(
        &self,
        name: &str,
        hops_remaining: usize,
    ) -> Result<Option<(NodeId, Node)>> {
        // Phase 1 — canonical name scan.
        {
            let rtxn = self.env.read_txn()?;
            for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
                let (key, node) = entry?;
                if node.qualified_name == name {
                    let mut id = [0u8; 32];
                    id.copy_from_slice(key);
                    return Ok(Some((NodeId(id), node)));
                }
            }
        }

        // Phase 2 — re-export facade fallback.
        if hops_remaining == 0 {
            return Ok(None);
        }

        if let Some(found) = self.lookup_impl_module_item_alias(name, hops_remaining)? {
            return Ok(Some(found));
        }

        let Some((prefix, leaf)) = name.rsplit_once("::") else {
            return Ok(None);
        };
        if prefix.is_empty() || leaf.is_empty() {
            return Ok(None);
        }

        let Some((prefix_id, _prefix_node)) =
            self.lookup_by_qualified_name_inner(prefix, hops_remaining - 1)?
        else {
            return Ok(None);
        };

        let rtxn = self.env.read_txn()?;
        for entry in self.bindings_for_from_module(&rtxn, prefix_id)? {
            let binding = entry?;
            if binding.visible_name != leaf {
                continue;
            }
            // A Declared binding for this name would already have surfaced in
            // Phase 1 via the target's canonical qualified_name. Skip it here so
            // the fallback is strictly about following re-export facades.
            if binding.kind == BindingKind::Declared {
                continue;
            }
            if let Some(target_node) =
                self.dbs.nodes_by_id.get(&rtxn, binding.target.as_bytes())?
            {
                return Ok(Some((binding.target, target_node)));
            }
        }
        Ok(None)
    }

    fn lookup_impl_module_item_alias(
        &self,
        name: &str,
        hops_remaining: usize,
    ) -> Result<Option<(NodeId, Node)>> {
        let Some((module_prefix, type_name, member_name)) = impl_module_item_alias_parts(name)
        else {
            return Ok(None);
        };

        let Some((_module_id, module_node)) =
            self.lookup_by_qualified_name_inner(module_prefix, hops_remaining - 1)?
        else {
            return Ok(None);
        };
        if module_node.kind != NodeKind::Module {
            return Ok(None);
        }

        let mut resolved: Option<(NodeId, Node)> = None;
        let rtxn = self.env.read_txn()?;
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            if !is_impl_module_item_alias_candidate(
                &node,
                module_node.crate_id,
                module_node.file.as_deref(),
                type_name,
                member_name,
            ) {
                continue;
            }

            let mut id = [0u8; 32];
            id.copy_from_slice(key);
            if resolved.is_some() {
                return Ok(None);
            }
            resolved = Some((NodeId(id), node));
        }

        Ok(resolved)
    }

    pub fn node_by_id(&self, rtxn: &RoTxn<'_, heed::WithoutTls>, id: NodeId) -> Result<Option<Node>> {
        Ok(self.dbs.nodes_by_id.get(rtxn, id.as_bytes())?)
    }

    /// Given a `Crate` node's id, find its root `Module` — the module whose
    /// `parent_id == Some(crate_id)` and whose `qualified_name` equals the
    /// crate's `qualified_name`. Returns `None` if the supplied id does not
    /// resolve to a `Crate`, or if no matching root module exists in this
    /// snapshot.
    ///
    /// Implementation note: `lookup_by_qualified_name` returns only the first
    /// match it finds while scanning `nodes_by_id`, but the crate node and its
    /// root module share the same `qualified_name`. This helper scans
    /// `nodes_by_id` looking for the (kind=Module, parent=crate, name=crate)
    /// triple, which is unique by construction in the extraction model.
    pub fn find_root_module_of(&self, crate_id: NodeId) -> Result<Option<NodeId>> {
        let rtxn = self.env.read_txn()?;
        let crate_node = match self.dbs.nodes_by_id.get(&rtxn, crate_id.as_bytes())? {
            Some(n) if n.kind == NodeKind::Crate => n,
            _ => return Ok(None),
        };
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            if node.kind == NodeKind::Module
                && node.parent_id == Some(crate_id)
                && node.qualified_name == crate_node.qualified_name
            {
                let mut id = [0u8; 32];
                id.copy_from_slice(key);
                return Ok(Some(NodeId(id)));
            }
        }
        Ok(None)
    }

    /// Distinct outgoing references from `caller_fn`'s body.
    ///
    /// Wraps the private `usages_for_consumer_function` iterator and dedupes
    /// by target `NodeId`. Includes calls, type references, const reads —
    /// anything `Usage` produces with `consumer_function == Some(caller_fn)`.
    /// The caller (codemap layer) classifies edges by reading each target's
    /// `Node.item_kind`.
    pub(in crate::graph) fn callees_of(&self, caller_fn: NodeId) -> Result<Vec<NodeId>> {
        let rtxn = self.env.read_txn()?;
        let mut seen: HashSet<NodeId> = HashSet::new();
        for entry in self.usages_for_consumer_function(&rtxn, caller_fn)? {
            seen.insert(entry?.target);
        }
        Ok(seen.into_iter().collect())
    }

    /// Distinct functions whose body contains a reference to `target`.
    ///
    /// Mirrors the `consumer_function.is_some()` filter used by `who_calls`.
    /// Semantics depend on `target`'s `ItemKind`: if `target` is callable
    /// these are callers, if `target` is a type these are consumers —
    /// classification is the caller's concern.
    pub(in crate::graph) fn referrers_of(&self, target: NodeId) -> Result<Vec<NodeId>> {
        let rtxn = self.env.read_txn()?;
        let mut seen: HashSet<NodeId> = HashSet::new();
        for entry in self.usages_for_target(&rtxn, target)? {
            if let Some(referrer) = entry?.consumer_function {
                seen.insert(referrer);
            }
        }
        Ok(seen.into_iter().collect())
    }
}
