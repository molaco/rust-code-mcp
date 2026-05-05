//! Phase 8 — `recursion_check`.
//!
//! Pure read-side query on the Layer 10 call graph data. Enumerates fns from
//! `signatures_by_target` (Phase 5) and traces outgoing call edges via the
//! `usages_by_consumer_function` DUP_SORT sub-DB (caller fn NodeId → UsageId,
//! one row per call site; the Usage record's `target` field is the callee).
//! For each fn we run a bounded DFS up to `max_cycle_length` looking for a
//! return to the start node. Cycles are canonicalized (rotated so the
//! lowest-id member is first) and deduped.

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use super::ids::NodeId;
use super::snapshot::OpenedSnapshot;

#[derive(Debug, Clone)]
pub struct RecursionOpts {
    pub crate_id_filter: Option<NodeId>,
    pub max_cycle_length: usize,
}

#[derive(Debug, Clone)]
pub struct RecursionCycleInternal {
    pub fns: Vec<NodeId>,
    pub cycle_length: usize,
    pub direct_recursion: bool,
}

pub const HARD_CAP_CYCLE_LENGTH: usize = 12;
pub const DEFAULT_CYCLE_LENGTH: usize = 5;

pub fn clamp_cycle_length(requested: Option<usize>) -> usize {
    let n = requested.unwrap_or(DEFAULT_CYCLE_LENGTH);
    n.clamp(1, HARD_CAP_CYCLE_LENGTH)
}

pub fn recursion_check(
    snap: &OpenedSnapshot,
    opts: RecursionOpts,
) -> Result<Vec<RecursionCycleInternal>> {
    let rtxn = snap.env.read_txn()?;

    let mut fn_ids: Vec<NodeId> = Vec::new();
    let mut fn_qnames: HashMap<NodeId, String> = HashMap::new();
    let mut in_scope: HashSet<NodeId> = HashSet::new();
    for entry in snap.dbs.signatures_by_target.iter(&rtxn)? {
        let (key, _sig) = entry?;
        let mut id = [0u8; 32];
        id.copy_from_slice(key);
        let target = NodeId(id);
        let Some(node) = snap.dbs.nodes_by_id.get(&rtxn, key)? else {
            continue;
        };
        let scoped = match opts.crate_id_filter {
            Some(filter_id) => node.crate_id == Some(filter_id),
            None => true,
        };
        if scoped {
            in_scope.insert(target);
        }
        fn_ids.push(target);
        fn_qnames.insert(target, node.qualified_name);
    }

    let mut adjacency: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for caller in &fn_ids {
        let mut callees: Vec<NodeId> = Vec::new();
        let mut seen: HashSet<NodeId> = HashSet::new();
        if let Some(iter) = snap
            .dbs
            .usages_by_consumer_function
            .get_duplicates(&rtxn, caller.as_bytes())?
        {
            for entry in iter {
                let (_k, uid_bytes) = entry?;
                let mut uid = [0u8; 32];
                uid.copy_from_slice(uid_bytes);
                let Some(usage) = snap.dbs.usages_by_id.get(&rtxn, &uid)? else {
                    continue;
                };
                if seen.insert(usage.target) {
                    callees.push(usage.target);
                }
            }
        }
        adjacency.insert(*caller, callees);
    }

    drop(rtxn);

    let outgoing = |node: NodeId| -> Vec<NodeId> {
        adjacency.get(&node).cloned().unwrap_or_default()
    };

    let mut canonical_set: HashSet<Vec<NodeId>> = HashSet::new();
    let mut canonical_cycles: Vec<Vec<NodeId>> = Vec::new();

    for start in &fn_ids {
        let cycles = find_cycles_from(*start, opts.max_cycle_length, &outgoing);
        for cycle in cycles {
            let canonical = canonicalize_cycle(cycle);
            if !canonical_set.contains(&canonical) {
                canonical_set.insert(canonical.clone());
                canonical_cycles.push(canonical);
            }
        }
    }

    let mut out: Vec<RecursionCycleInternal> = canonical_cycles
        .into_iter()
        .filter(|cycle| {
            if opts.crate_id_filter.is_none() {
                return true;
            }
            cycle.iter().any(|id| in_scope.contains(id))
        })
        .map(|cycle| {
            let cycle_length = cycle.len();
            RecursionCycleInternal {
                direct_recursion: cycle_length == 1,
                cycle_length,
                fns: cycle,
            }
        })
        .collect();

    out.sort_by(|a, b| {
        a.cycle_length.cmp(&b.cycle_length).then_with(|| {
            let a_name = a.fns.first().and_then(|id| fn_qnames.get(id)).cloned().unwrap_or_default();
            let b_name = b.fns.first().and_then(|id| fn_qnames.get(id)).cloned().unwrap_or_default();
            a_name.cmp(&b_name)
        })
    });

    Ok(out)
}

/// Find all simple cycles starting from `start` that return to `start` within
/// `max_depth` hops, following outgoing edges produced by `outgoing_edges`.
/// Returns each cycle as `Vec<NodeId>` ordered by visit (cycle[0] == start);
/// the closing edge back to `start` is implicit and not appended.
///
/// Pure function over the closure — unit-testable against a hand-built
/// adjacency list.
pub fn find_cycles_from<F>(start: NodeId, max_depth: usize, outgoing_edges: F) -> Vec<Vec<NodeId>>
where
    F: Fn(NodeId) -> Vec<NodeId>,
{
    if max_depth == 0 {
        return Vec::new();
    }
    let mut out: Vec<Vec<NodeId>> = Vec::new();
    let mut path: Vec<NodeId> = vec![start];
    let mut on_path: HashSet<NodeId> = HashSet::new();
    on_path.insert(start);
    dfs(start, &outgoing_edges, max_depth, &mut path, &mut on_path, &mut out);
    out
}

fn dfs<F>(
    start: NodeId,
    outgoing: &F,
    max_depth: usize,
    path: &mut Vec<NodeId>,
    on_path: &mut HashSet<NodeId>,
    out: &mut Vec<Vec<NodeId>>,
) where
    F: Fn(NodeId) -> Vec<NodeId>,
{
    if path.len() > max_depth {
        return;
    }
    let current = *path.last().expect("path is non-empty");
    for next in outgoing(current) {
        if next == start {
            out.push(path.clone());
            continue;
        }
        if on_path.contains(&next) {
            continue;
        }
        if path.len() >= max_depth {
            continue;
        }
        path.push(next);
        on_path.insert(next);
        dfs(start, outgoing, max_depth, path, on_path, out);
        on_path.remove(&next);
        path.pop();
    }
}

/// Rotate a cycle so its lowest-id `NodeId` (by lexicographic byte order)
/// comes first. Used for dedup: two cycles with the same canonical form are
/// the same cycle viewed from different starting nodes.
pub fn canonicalize_cycle(mut cycle: Vec<NodeId>) -> Vec<NodeId> {
    if cycle.len() <= 1 {
        return cycle;
    }
    let mut min_idx = 0usize;
    for (i, id) in cycle.iter().enumerate().skip(1) {
        if id.as_bytes() < cycle[min_idx].as_bytes() {
            min_idx = i;
        }
    }
    cycle.rotate_left(min_idx);
    cycle
}

pub fn enclosing_fn_qualified_names(
    snap: &OpenedSnapshot,
    cycle: &[NodeId],
) -> Result<Vec<String>> {
    let rtxn = snap.env.read_txn()?;
    let mut out: Vec<String> = Vec::with_capacity(cycle.len());
    for id in cycle {
        let qn = snap
            .dbs
            .nodes_by_id
            .get(&rtxn, id.as_bytes())?
            .map(|n| n.qualified_name)
            .unwrap_or_default();
        out.push(qn);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn nid(byte0: u8) -> NodeId {
        let mut bytes = [0u8; 32];
        bytes[0] = byte0;
        NodeId(bytes)
    }

    fn adj_closure(map: HashMap<NodeId, Vec<NodeId>>) -> impl Fn(NodeId) -> Vec<NodeId> {
        move |n: NodeId| map.get(&n).cloned().unwrap_or_default()
    }

    #[test]
    fn self_loop_emits_one_cycle_of_length_one() {
        let a = nid(1);
        let mut g = HashMap::new();
        g.insert(a, vec![a]);
        let cycles = find_cycles_from(a, 5, adj_closure(g));
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0], vec![a]);
    }

    #[test]
    fn mutual_two_cycle() {
        let a = nid(1);
        let b = nid(2);
        let mut g = HashMap::new();
        g.insert(a, vec![b]);
        g.insert(b, vec![a]);
        let cycles = find_cycles_from(a, 5, adj_closure(g));
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0], vec![a, b]);
    }

    #[test]
    fn three_cycle() {
        let a = nid(1);
        let b = nid(2);
        let c = nid(3);
        let mut g = HashMap::new();
        g.insert(a, vec![b]);
        g.insert(b, vec![c]);
        g.insert(c, vec![a]);
        let cycles = find_cycles_from(a, 5, adj_closure(g));
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0], vec![a, b, c]);
    }

    #[test]
    fn no_cycle_terminal() {
        let a = nid(1);
        let b = nid(2);
        let mut g = HashMap::new();
        g.insert(a, vec![b]);
        let cycles = find_cycles_from(a, 5, adj_closure(g));
        assert!(cycles.is_empty());
    }

    #[test]
    fn depth_bounded_excludes_long_cycle() {
        let a = nid(1);
        let b = nid(2);
        let c = nid(3);
        let d = nid(4);
        let mut g = HashMap::new();
        g.insert(a, vec![b]);
        g.insert(b, vec![c]);
        g.insert(c, vec![d]);
        g.insert(d, vec![a]);
        let cycles = find_cycles_from(a, 3, adj_closure(g));
        assert!(cycles.is_empty());
    }

    #[test]
    fn branching_emits_two_cycles() {
        let a = nid(1);
        let b = nid(2);
        let c = nid(3);
        let mut g = HashMap::new();
        g.insert(a, vec![b, c]);
        g.insert(b, vec![a]);
        g.insert(c, vec![a]);
        let cycles = find_cycles_from(a, 5, adj_closure(g));
        assert_eq!(cycles.len(), 2);
        assert!(cycles.contains(&vec![a, b]));
        assert!(cycles.contains(&vec![a, c]));
    }

    #[test]
    fn canonicalize_rotates_to_lowest_first() {
        let a = nid(1);
        let b = nid(2);
        let c = nid(3);
        let got = canonicalize_cycle(vec![b, c, a]);
        assert_eq!(got, vec![a, b, c]);
    }

    #[test]
    fn canonicalize_single_element_is_identity() {
        let a = nid(7);
        let got = canonicalize_cycle(vec![a]);
        assert_eq!(got, vec![a]);
    }
}
