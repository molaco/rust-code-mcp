//! Shared low-level helpers used across multiple `query/*` family files.
//!
//! These are the iteration primitives over LMDB indices (`bindings_for_*`,
//! `usages_for_*`) plus the `dependency_node_for` resolver and re-export
//! traversal depth limit. Separated from `queries.rs` in PR 11 review
//! fix-up so the legacy module can be a true facade.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use heed::RoTxn;

use super::super::ids::{BindingId, NodeId};
use super::super::model::{Binding, BindingKind, BindingVisibility, Node, NodeKind, Usage};
use super::super::snapshot::OpenedSnapshot;
use super::model::OverlapScope;

/// Maximum re-export facade hops to follow before giving up. Bounds recursion
/// in the (pathological) case of a binding chain or a self-referential cycle.
pub(in crate::graph) const MAX_REEXPORT_HOPS: usize = 8;

pub(in crate::graph) fn declared_visibility_map(
    snap: &OpenedSnapshot,
    rtxn: &RoTxn<'_, heed::WithoutTls>,
    target_ids: &HashSet<NodeId>,
) -> Result<HashMap<NodeId, String>> {
    let mut target_parents: HashMap<NodeId, NodeId> = HashMap::new();
    for target_id in target_ids {
        if let Some(node) = snap.dbs.nodes_by_id.get(rtxn, target_id.as_bytes())? {
            if let Some(parent) = node.parent_id {
                target_parents.insert(*target_id, parent);
            }
        }
    }
    let mut visibility_picks: HashMap<NodeId, (BindingVisibility, bool)> = HashMap::new();
    for entry in snap.dbs.bindings_by_id.iter(rtxn)? {
        let (_k, binding) = entry?;
        if binding.kind != BindingKind::Declared {
            continue;
        }
        if !target_ids.contains(&binding.target) {
            continue;
        }
        let parent_match = target_parents
            .get(&binding.target)
            .map(|p| *p == binding.from_module)
            .unwrap_or(false);
        match visibility_picks.get(&binding.target) {
            None => {
                visibility_picks.insert(binding.target, (binding.visibility, parent_match));
            }
            Some((_, existing_parent_match)) => {
                if !existing_parent_match && parent_match {
                    visibility_picks.insert(binding.target, (binding.visibility, parent_match));
                }
            }
        }
    }
    let mut visibility: HashMap<NodeId, String> = HashMap::new();
    for (id, (vis, _)) in visibility_picks {
        visibility.insert(id, format_binding_visibility(rtxn, snap, vis));
    }
    Ok(visibility)
}

pub(in crate::graph) fn format_binding_visibility(
    rtxn: &RoTxn<'_, heed::WithoutTls>,
    snap: &OpenedSnapshot,
    vis: BindingVisibility,
) -> String {
    match vis {
        BindingVisibility::Public => "pub".to_string(),
        BindingVisibility::Private => "pub(self)".to_string(),
        BindingVisibility::Crate(_) => "pub(crate)".to_string(),
        BindingVisibility::RestrictedTo(id) => {
            match snap.dbs.nodes_by_id.get(rtxn, id.as_bytes()).ok().flatten() {
                Some(node) => format!("pub(in {})", node.qualified_name),
                None => "pub(in ?)".to_string(),
            }
        }
    }
}

pub(in crate::graph) fn crate_scope_allows(
    scope: OverlapScope,
    crate_id: NodeId,
    crate_target_kind_for: &HashMap<NodeId, String>,
    vendor_crates: &HashSet<NodeId>,
) -> bool {
    match scope {
        OverlapScope::All => true,
        OverlapScope::Local | OverlapScope::LocalNoVendor => {
            let target_kind = crate_target_kind_for
                .get(&crate_id)
                .map(String::as_str)
                .unwrap_or("lib");
            let local_target = matches!(target_kind, "lib" | "bin");
            local_target && (scope == OverlapScope::Local || !vendor_crates.contains(&crate_id))
        }
    }
}

impl OpenedSnapshot {
    pub(super) fn bindings_for_from_module<'txn>(
        &'txn self,
        rtxn: &'txn RoTxn<'_, heed::WithoutTls>,
        module: NodeId,
    ) -> Result<impl Iterator<Item = Result<Binding>> + 'txn> {
        // bindings_by_from_module is DUP_SORT: NodeId → BindingId. We iterate
        // duplicates of the given key, then resolve each BindingId to a Binding.
        Ok(self
            .dbs
            .bindings_by_from_module
            .get_duplicates(rtxn, module.as_bytes())?
            .into_iter()
            .flatten()
            .map(move |entry| {
                let (_k, bid_bytes) = entry?;
                let mut bid = [0u8; 32];
                bid.copy_from_slice(bid_bytes);
                let binding = self
                    .dbs
                    .bindings_by_id
                    .get(rtxn, &bid)?
                    .context("dangling BindingId in bindings_by_from_module")?;
                let _ = BindingId(bid);
                Ok(binding)
            }))
    }

    pub(super) fn bindings_for_target<'txn>(
        &'txn self,
        rtxn: &'txn RoTxn<'_, heed::WithoutTls>,
        target: NodeId,
    ) -> Result<impl Iterator<Item = Result<Binding>> + 'txn> {
        Ok(self
            .dbs
            .bindings_by_target
            .get_duplicates(rtxn, target.as_bytes())?
            .into_iter()
            .flatten()
            .map(move |entry| {
                let (_k, bid_bytes) = entry?;
                let mut bid = [0u8; 32];
                bid.copy_from_slice(bid_bytes);
                let binding = self
                    .dbs
                    .bindings_by_id
                    .get(rtxn, &bid)?
                    .context("dangling BindingId in bindings_by_target")?;
                Ok(binding)
            }))
    }

    pub(super) fn usages_for_target<'txn>(
        &'txn self,
        rtxn: &'txn RoTxn<'_, heed::WithoutTls>,
        target: NodeId,
    ) -> Result<impl Iterator<Item = Result<Usage>> + 'txn> {
        Ok(self
            .dbs
            .usages_by_target
            .get_duplicates(rtxn, target.as_bytes())?
            .into_iter()
            .flatten()
            .map(move |entry| {
                let (_k, uid_bytes) = entry?;
                let mut uid = [0u8; 32];
                uid.copy_from_slice(uid_bytes);
                let usage = self
                    .dbs
                    .usages_by_id
                    .get(rtxn, &uid)?
                    .context("dangling UsageId in usages_by_target")?;
                Ok(usage)
            }))
    }

    pub(super) fn usages_for_consumer<'txn>(
        &'txn self,
        rtxn: &'txn RoTxn<'_, heed::WithoutTls>,
        consumer: NodeId,
    ) -> Result<impl Iterator<Item = Result<Usage>> + 'txn> {
        Ok(self
            .dbs
            .usages_by_consumer
            .get_duplicates(rtxn, consumer.as_bytes())?
            .into_iter()
            .flatten()
            .map(move |entry| {
                let (_k, uid_bytes) = entry?;
                let mut uid = [0u8; 32];
                uid.copy_from_slice(uid_bytes);
                let usage = self
                    .dbs
                    .usages_by_id
                    .get(rtxn, &uid)?
                    .context("dangling UsageId in usages_by_consumer")?;
                Ok(usage)
            }))
    }

    pub(super) fn usages_for_consumer_function<'txn>(
        &'txn self,
        rtxn: &'txn RoTxn<'_, heed::WithoutTls>,
        caller_fn: NodeId,
    ) -> Result<impl Iterator<Item = Result<Usage>> + 'txn> {
        Ok(self
            .dbs
            .usages_by_consumer_function
            .get_duplicates(rtxn, caller_fn.as_bytes())?
            .into_iter()
            .flatten()
            .map(move |entry| {
                let (_k, uid_bytes) = entry?;
                let mut uid = [0u8; 32];
                uid.copy_from_slice(uid_bytes);
                let usage = self
                    .dbs
                    .usages_by_id
                    .get(rtxn, &uid)?
                    .context("dangling UsageId in usages_by_consumer_function")?;
                Ok(usage)
            }))
    }
}

pub(in crate::graph) fn dependency_node_for(
    nodes: &HashMap<NodeId, Node>,
    target: NodeId,
) -> Option<(NodeId, &Node)> {
    let mut current = target;
    let mut guard = 0usize;
    loop {
        let node = nodes.get(&current)?;
        match node.kind {
            NodeKind::Module | NodeKind::Crate | NodeKind::ExternalSymbol => {
                return Some((current, node));
            }
            NodeKind::Workspace => return None,
            NodeKind::Item => {
                current = node.parent_id?;
                guard += 1;
                if guard > 32 {
                    return None;
                }
            }
        }
    }
}
