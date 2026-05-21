//! Query methods on `OpenedSnapshot` — calls family.
//!
//! Covers call-graph queries: `who_calls`, `calls_from`, `call_graph`,
//! `callers_in_crate`, `recursive_callers_count`. Moved here from
//! `graph::queries` in PR 09.

use std::collections::HashSet;

use anyhow::Result;

use super::super::ids::NodeId;
use super::super::labels::usage_category_label;
use super::super::model::Usage;
use super::super::snapshot::OpenedSnapshot;
use super::model::{CallGraphNode, EnrichedCallSite, RecursiveCallersCount};

impl OpenedSnapshot {
    /// Every non-import reference to `target_fn` whose call site is inside
    /// some function body. Returns the caller's qualified name + file:byte
    /// range + category. References from non-fn scopes (const initializers,
    /// trait bounds, enum discriminants) are excluded — see `who_uses` for
    /// those.
    pub fn who_calls(&self, target_fn: NodeId) -> Result<Vec<EnrichedCallSite>> {
        let rtxn = self.env.read_txn()?;
        let callee_qualified_name = self
            .dbs
            .nodes_by_id
            .get(&rtxn, target_fn.as_bytes())?
            .map(|n| n.qualified_name)
            .unwrap_or_default();
        let mut usages: Vec<Usage> = Vec::new();
        for entry in self.usages_for_target(&rtxn, target_fn)? {
            let usage = entry?;
            if usage.consumer_function.is_some() {
                usages.push(usage);
            }
        }

        let mut out = Vec::with_capacity(usages.len());
        for usage in usages {
            let caller_qualified_name = match usage.consumer_function {
                Some(fn_id) => self
                    .dbs
                    .nodes_by_id
                    .get(&rtxn, fn_id.as_bytes())?
                    .map(|n| n.qualified_name),
                None => None,
            };
            out.push(EnrichedCallSite {
                caller_qualified_name,
                callee_qualified_name: callee_qualified_name.clone(),
                file: usage.file,
                start: usage.start,
                end: usage.end,
                category: usage_category_label(usage.category).to_string(),
            });
        }
        Ok(out)
    }

    /// Every non-import reference made *from* the body of `caller_fn`. Returns
    /// the callee's qualified name + file:byte range + category. Closures
    /// inside `caller_fn` attribute to it (RA's default for
    /// `SemanticsScope::containing_function`).
    pub fn calls_from(&self, caller_fn: NodeId) -> Result<Vec<EnrichedCallSite>> {
        let rtxn = self.env.read_txn()?;
        let caller_qualified_name = self
            .dbs
            .nodes_by_id
            .get(&rtxn, caller_fn.as_bytes())?
            .map(|n| n.qualified_name);

        let mut usages: Vec<Usage> = Vec::new();
        for entry in self.usages_for_consumer_function(&rtxn, caller_fn)? {
            usages.push(entry?);
        }

        let mut out = Vec::with_capacity(usages.len());
        for usage in usages {
            let callee_qualified_name = self
                .dbs
                .nodes_by_id
                .get(&rtxn, usage.target.as_bytes())?
                .map(|n| n.qualified_name)
                .unwrap_or_default();
            out.push(EnrichedCallSite {
                caller_qualified_name: caller_qualified_name.clone(),
                callee_qualified_name,
                file: usage.file,
                start: usage.start,
                end: usage.end,
                category: usage_category_label(usage.category).to_string(),
            });
        }
        Ok(out)
    }

    /// Bounded recursive descent over outgoing fn-body references rooted at
    /// `root_fn`. At each node, `calls_from(node)` is computed, distinct callee
    /// NodeIds are recursed into, and `depth` is decremented. A global
    /// `visited: HashSet<NodeId>` prevents re-expanding the same fn twice
    /// anywhere in the tree (so cycles and DAG-style fan-in both terminate).
    ///
    /// `truncated_at_cycle` flags subtrees pruned because the callee was
    /// already expanded elsewhere — the same callees would have appeared.
    /// `truncated_at_depth` flags subtrees pruned because `depth == 0` and
    /// the node has at least one outgoing edge.
    pub fn call_graph(&self, root_fn: NodeId, depth: u32) -> Result<CallGraphNode> {
        let mut visited: HashSet<NodeId> = HashSet::new();
        self.call_graph_rec(root_fn, depth, &mut visited)
    }

    fn call_graph_rec(
        &self,
        fn_id: NodeId,
        depth: u32,
        visited: &mut HashSet<NodeId>,
    ) -> Result<CallGraphNode> {
        let rtxn = self.env.read_txn()?;
        let node = self.dbs.nodes_by_id.get(&rtxn, fn_id.as_bytes())?;
        let (fn_qualified_name, crate_name) = match node {
            Some(n) => {
                let crate_name = match n.crate_id {
                    Some(cid) => self
                        .dbs
                        .nodes_by_id
                        .get(&rtxn, cid.as_bytes())?
                        .map(|c| c.qualified_name),
                    None => None,
                };
                (n.qualified_name, crate_name)
            }
            None => (String::new(), None),
        };
        drop(rtxn);

        // If this fn has been expanded somewhere else already, prune.
        // The root call (visited empty) always proceeds; subsequent visits to
        // the same NodeId from anywhere in the tree become cycle-truncated.
        if !visited.insert(fn_id) {
            return Ok(CallGraphNode {
                fn_qualified_name,
                crate_name,
                callees: Vec::new(),
                truncated_at_cycle: true,
                truncated_at_depth: false,
            });
        }

        // Collect distinct callee NodeIds. `usages_for_consumer_function`
        // returns one row per call site, so the same callee NodeId may appear
        // multiple times.
        let rtxn2 = self.env.read_txn()?;
        let mut distinct_callees: Vec<NodeId> = Vec::new();
        let mut seen: HashSet<NodeId> = HashSet::new();
        for entry in self.usages_for_consumer_function(&rtxn2, fn_id)? {
            let usage = entry?;
            if seen.insert(usage.target) {
                distinct_callees.push(usage.target);
            }
        }
        drop(rtxn2);

        // At depth 0, leave callees empty and flag truncation if there were any.
        if depth == 0 {
            return Ok(CallGraphNode {
                fn_qualified_name,
                crate_name,
                callees: Vec::new(),
                truncated_at_cycle: false,
                truncated_at_depth: !distinct_callees.is_empty(),
            });
        }

        let mut callees: Vec<CallGraphNode> = Vec::with_capacity(distinct_callees.len());
        for callee_id in distinct_callees {
            let child = self.call_graph_rec(callee_id, depth - 1, visited)?;
            callees.push(child);
        }

        Ok(CallGraphNode {
            fn_qualified_name,
            crate_name,
            callees,
            truncated_at_cycle: false,
            truncated_at_depth: false,
        })
    }

    /// `who_calls(target)` filtered to call sites whose *caller fn* lives in a
    /// crate whose qualified_name equals `crate_qualified`. Callers in any
    /// other crate (or with a missing `crate_id`) are dropped. Note: this
    /// filters by the **caller's** crate, not the target's.
    pub fn callers_in_crate(
        &self,
        target: NodeId,
        crate_qualified: &str,
    ) -> Result<Vec<EnrichedCallSite>> {
        let rtxn = self.env.read_txn()?;
        let callee_qualified_name = self
            .dbs
            .nodes_by_id
            .get(&rtxn, target.as_bytes())?
            .map(|n| n.qualified_name)
            .unwrap_or_default();

        let mut out: Vec<EnrichedCallSite> = Vec::new();
        for entry in self.usages_for_target(&rtxn, target)? {
            let usage = entry?;
            let Some(fn_id) = usage.consumer_function else {
                continue;
            };
            let Some(caller_node) = self.dbs.nodes_by_id.get(&rtxn, fn_id.as_bytes())? else {
                continue;
            };
            let Some(crate_id) = caller_node.crate_id else {
                continue;
            };
            let Some(crate_node) = self.dbs.nodes_by_id.get(&rtxn, crate_id.as_bytes())? else {
                continue;
            };
            if crate_node.qualified_name != crate_qualified {
                continue;
            }
            out.push(EnrichedCallSite {
                caller_qualified_name: Some(caller_node.qualified_name),
                callee_qualified_name: callee_qualified_name.clone(),
                file: usage.file,
                start: usage.start,
                end: usage.end,
                category: usage_category_label(usage.category).to_string(),
            });
        }
        Ok(out)
    }

    /// Reverse BFS from `target`: count distinct caller fns reachable backward
    /// up to `depth` hops. depth=0 returns zeros. depth=1 returns just the
    /// direct callers. Higher depths include transitive callers (callers of
    /// callers, etc.). Counts *fns*, not call sites — a fn that calls target
    /// 5 times counts as 1 caller.
    pub fn recursive_callers_count(
        &self,
        target: NodeId,
        depth: u32,
    ) -> Result<RecursiveCallersCount> {
        let rtxn = self.env.read_txn()?;
        let target_qualified_name = self
            .dbs
            .nodes_by_id
            .get(&rtxn, target.as_bytes())?
            .map(|n| n.qualified_name)
            .unwrap_or_default();
        drop(rtxn);

        if depth == 0 {
            return Ok(RecursiveCallersCount {
                target_qualified_name,
                depth: 0,
                direct_callers: 0,
                transitive_callers: 0,
                depth_reached: 0,
                truncated_at_depth: false,
            });
        }

        // Direct callers (hop 1).
        let mut visited: HashSet<NodeId> = HashSet::new();
        let mut frontier: Vec<NodeId> = Vec::new();
        {
            let rtxn = self.env.read_txn()?;
            for entry in self.usages_for_target(&rtxn, target)? {
                let usage = entry?;
                if let Some(fn_id) = usage.consumer_function {
                    if visited.insert(fn_id) {
                        frontier.push(fn_id);
                    }
                }
            }
        }
        let direct_callers = visited.len();
        let mut depth_reached: u32 = if direct_callers > 0 { 1 } else { 0 };

        // Hops 2..=depth.
        let mut hop: u32 = 1;
        let mut truncated_at_depth = false;
        while hop < depth && !frontier.is_empty() {
            let mut next: Vec<NodeId> = Vec::new();
            for fn_id in frontier.drain(..) {
                let rtxn = self.env.read_txn()?;
                for entry in self.usages_for_target(&rtxn, fn_id)? {
                    let usage = entry?;
                    if let Some(caller_id) = usage.consumer_function {
                        if visited.insert(caller_id) {
                            next.push(caller_id);
                        }
                    }
                }
            }
            if !next.is_empty() {
                depth_reached = hop + 1;
            }
            frontier = next;
            hop += 1;
        }
        // If we exited because we hit depth and the frontier still has
        // un-visited would-be expansions, flag truncation.
        if hop == depth && !frontier.is_empty() {
            // Any node in the frontier that itself has at least one un-visited
            // caller means the BFS isn't exhausted. Do a single peek pass.
            'outer: for fn_id in &frontier {
                let rtxn = self.env.read_txn()?;
                for entry in self.usages_for_target(&rtxn, *fn_id)? {
                    let usage = entry?;
                    if let Some(caller_id) = usage.consumer_function {
                        if !visited.contains(&caller_id) {
                            truncated_at_depth = true;
                            break 'outer;
                        }
                    }
                }
            }
        }

        Ok(RecursiveCallersCount {
            target_qualified_name,
            depth,
            direct_callers,
            transitive_callers: visited.len(),
            depth_reached,
            truncated_at_depth,
        })
    }
}
