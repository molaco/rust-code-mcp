//! Shared low-level helpers used across multiple `query/*` family files.
//!
//! These are the iteration primitives over LMDB indices (`bindings_for_*`,
//! `usages_for_*`) plus the `dependency_node_for` resolver and re-export
//! traversal depth limit. Separated from `queries.rs` in PR 11 review
//! fix-up so the legacy module can be a true facade.

use std::collections::HashMap;

use anyhow::{Context, Result};
use heed::RoTxn;

use super::super::ids::{BindingId, NodeId};
use super::super::model::{Binding, Node, NodeKind, Usage};
use super::super::snapshot::OpenedSnapshot;

/// Maximum re-export facade hops to follow before giving up. Bounds recursion
/// in the (pathological) case of a binding chain or a self-referential cycle.
pub(in crate::graph) const MAX_REEXPORT_HOPS: usize = 8;

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
