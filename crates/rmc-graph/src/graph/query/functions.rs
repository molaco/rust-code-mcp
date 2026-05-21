//! Query methods on `OpenedSnapshot` — functions family.
//!
//! Covers function-signature queries: `function_signature`,
//! `functions_with_filter`. Moved here from `graph::queries` in PR 11.

use anyhow::Result;

use super::super::ids::NodeId;
use super::super::model::{FunctionSignature, SelfKind};
use super::super::snapshot::OpenedSnapshot;
use super::model::{FunctionFilter, FunctionWithSignature, SelfKindFilter};

impl OpenedSnapshot {
    /// v9: return the recorded `FunctionSignature` for `target` (a local
    /// function NodeId), or `None` if no signature is present (e.g. the
    /// target isn't a function, or extraction skipped it). Single-key LMDB
    /// lookup, no scan.
    pub fn function_signature(&self, target: NodeId) -> Result<Option<FunctionSignature>> {
        let rtxn = self.env.read_txn()?;
        Ok(self.dbs.signatures_by_target.get(&rtxn, target.as_bytes())?)
    }

    /// v9: every local function in `crate_id` whose `FunctionSignature`
    /// matches every `Some` field of `filter`. Iterates the
    /// `signatures_by_target` table (linear in #fns), fetches the Node for
    /// each key to scope by `crate_id`, then applies the filter predicates.
    /// Sorted by qualified name.
    pub fn functions_with_filter(
        &self,
        crate_id: NodeId,
        filter: &FunctionFilter,
    ) -> Result<Vec<FunctionWithSignature>> {
        let rtxn = self.env.read_txn()?;
        let mut out: Vec<FunctionWithSignature> = Vec::new();
        for entry in self.dbs.signatures_by_target.iter(&rtxn)? {
            let (key, sig) = entry?;
            let mut id = [0u8; 32];
            id.copy_from_slice(key);
            let target = NodeId(id);
            let Some(node) = self.dbs.nodes_by_id.get(&rtxn, key)? else {
                continue;
            };
            if node.crate_id != Some(crate_id) {
                continue;
            }
            if !filter_matches(filter, &sig) {
                continue;
            }
            out.push(FunctionWithSignature {
                target,
                qualified_name: node.qualified_name,
                signature: sig,
            });
        }
        out.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
        Ok(out)
    }
}

/// v9: predicate for `functions_with_filter`. Every `Some` field on the
/// filter narrows the match; a `None` field is a no-op. Substring matches
/// (`has_param_type`, `returns_type_pattern`) are case-sensitive against
/// the HirDisplay strings in the signature.
fn filter_matches(filter: &FunctionFilter, sig: &FunctionSignature) -> bool {
    if let Some(want) = filter.is_async
        && sig.is_async != want
    {
        return false;
    }
    if let Some(min) = filter.min_param_count
        && sig.params.len() < min
    {
        return false;
    }
    if let Some(needle) = filter.has_param_type.as_deref()
        && !sig.params.iter().any(|p| p.ty.contains(needle))
    {
        return false;
    }
    if let Some(needle) = filter.returns_type_pattern.as_deref()
        && !sig.return_type.contains(needle)
    {
        return false;
    }
    if let Some(want) = filter.self_kind {
        let actual = sig.self_param;
        let ok = match want {
            SelfKindFilter::None => actual.is_none(),
            SelfKindFilter::Owned => matches!(actual, Some(SelfKind::Owned)),
            SelfKindFilter::Ref => matches!(actual, Some(SelfKind::Ref)),
            SelfKindFilter::RefMut => matches!(actual, Some(SelfKind::RefMut)),
        };
        if !ok {
            return false;
        }
    }
    true
}
