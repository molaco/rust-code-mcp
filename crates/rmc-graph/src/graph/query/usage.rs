//! Query methods on `OpenedSnapshot` — usage family.
//!
//! Covers who-imports / who-uses queries: `who_imports`, `usages_of`,
//! `usages_in`, `who_uses_summary`. Moved here from `graph::queries` in
//! PR 09.

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;

use super::super::ids::NodeId;
use super::super::labels::usage_category_label;
use super::super::model::{Binding, BindingKind, Usage};
use super::super::snapshot::OpenedSnapshot;
use super::model::UsageSummaryRow;

impl OpenedSnapshot {
    /// All bindings in the workspace whose target is `target` (and that aren't
    /// the target's own declaration). Useful for "who imports symbol X".
    pub fn who_imports(&self, target: NodeId) -> Result<Vec<Binding>> {
        let rtxn = self.env.read_txn()?;
        let mut out = Vec::new();
        for entry in self.bindings_for_target(&rtxn, target)? {
            let binding = entry?;
            if binding.kind != BindingKind::Declared {
                out.push(binding);
            }
        }
        Ok(out)
    }

    /// All non-import references to `target`, as recorded by `extract_usages`.
    /// `IMPORT` references are filtered at extraction time — they're modeled
    /// as `Binding`s instead. Order is unspecified.
    pub fn usages_of(&self, target: NodeId) -> Result<Vec<Usage>> {
        let rtxn = self.env.read_txn()?;
        let mut out = Vec::new();
        for entry in self.usages_for_target(&rtxn, target)? {
            out.push(entry?);
        }
        Ok(out)
    }

    /// All non-import references whose enclosing module is `consumer_module`.
    pub fn usages_in(&self, consumer_module: NodeId) -> Result<Vec<Usage>> {
        let rtxn = self.env.read_txn()?;
        let mut out = Vec::new();
        for entry in self.usages_for_consumer(&rtxn, consumer_module)? {
            out.push(entry?);
        }
        Ok(out)
    }

    /// Aggregation rollup of `usages_of(target)` grouped by `consumer_module`.
    /// Each row carries a total count and a per-category breakdown
    /// (Read/Write/Test/Other → count). Local inherent method calls and local
    /// trait-declaration dispatch are captured as Method items; remaining
    /// blind spots are indirect calls RA cannot resolve to a workspace Item
    /// (for example `dyn Trait` over external traits or generic `F: Fn(..)`).
    /// Sorted by `total_count` desc, ties broken by `consumer_qualified_name`.
    pub fn who_uses_summary(&self, target: NodeId) -> Result<Vec<UsageSummaryRow>> {
        let rtxn = self.env.read_txn()?;

        // Group by consumer_module: total + per-category breakdown.
        let mut totals: HashMap<NodeId, usize> = HashMap::new();
        let mut breakdown: HashMap<NodeId, BTreeMap<String, usize>> = HashMap::new();
        for entry in self.usages_for_target(&rtxn, target)? {
            let usage = entry?;
            *totals.entry(usage.consumer_module).or_insert(0) += 1;
            let cat = usage_category_label(usage.category).to_string();
            *breakdown
                .entry(usage.consumer_module)
                .or_default()
                .entry(cat)
                .or_insert(0) += 1;
        }

        // Resolve display names. We need the consumer module's qualified_name
        // and (separately) its crate's qualified_name for downstream display.
        let mut rows: Vec<UsageSummaryRow> = Vec::with_capacity(totals.len());
        for (consumer_module, total_count) in totals {
            let (qualified_name, crate_qualified) = match self
                .dbs
                .nodes_by_id
                .get(&rtxn, consumer_module.as_bytes())?
            {
                Some(node) => {
                    let crate_qual = match node.crate_id {
                        Some(cid) => self
                            .dbs
                            .nodes_by_id
                            .get(&rtxn, cid.as_bytes())?
                            .map(|n| n.qualified_name),
                        None => None,
                    };
                    (node.qualified_name, crate_qual)
                }
                None => (String::new(), None),
            };
            rows.push(UsageSummaryRow {
                consumer_qualified_name: qualified_name,
                consumer_crate: crate_qualified,
                total_count,
                category_breakdown: breakdown.remove(&consumer_module).unwrap_or_default(),
            });
        }
        rows.sort_by(|a, b| {
            b.total_count
                .cmp(&a.total_count)
                .then_with(|| a.consumer_qualified_name.cmp(&b.consumer_qualified_name))
        });
        Ok(rows)
    }
}
