//! Query methods on `OpenedSnapshot` — crates family.
//!
//! Covers crate-graph queries: `crate_edges`, `crate_dependency_metric`,
//! `forbidden_dependency_check`. Moved here from `graph::queries` in PR 10.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::Result;

use super::super::ids::NodeId;
use super::super::labels::{
    binding_kind_label as label_binding_kind, item_kind_short_label as label_item_kind,
    node_kind_label,
};
use super::super::model::{BindingKind, ItemKind, NodeKind};
use super::super::snapshot::OpenedSnapshot;
use super::model::{
    CrateEdge, CrateMetric, EdgeSymbol, ForbiddenDependencyRule, ForbiddenDependencyViolation,
};

impl OpenedSnapshot {
    /// Phase 4c: per-local-crate Robert Martin instability +
    /// abstractness metrics. Both metrics are NaN-guarded; degenerate
    /// counts (zero edges or zero items) return 0.0.
    pub fn crate_dependency_metric(&self) -> Result<Vec<CrateMetric>> {
        let edges = self.crate_edges()?;
        let rtxn = self.env.read_txn()?;

        // Collect every local crate: (crate_id, crate_name).
        let mut crates: Vec<(NodeId, String)> = Vec::new();
        // Per-crate item counters: (total_items, traits, pub_type_aliases).
        let mut item_counts: HashMap<NodeId, (u32, u32, u32)> = HashMap::new();
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            let mut id = [0u8; 32];
            id.copy_from_slice(key);
            let nid = NodeId(id);
            if node.kind == NodeKind::Crate {
                crates.push((nid, node.qualified_name.clone()));
            } else if node.kind == NodeKind::Item {
                if let Some(crate_id) = node.crate_id {
                    let counts = item_counts.entry(crate_id).or_insert((0, 0, 0));
                    counts.0 += 1;
                    match node.item_kind {
                        Some(ItemKind::Trait) => counts.1 += 1,
                        Some(ItemKind::TypeAlias) => {
                            // Only count pub type aliases for the abstractness ratio.
                            if node.visibility.as_deref() == Some("pub") {
                                counts.2 += 1;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Build distinct producer/consumer sets per local crate name.
        // crate_edges keys by qualified name string, so we map names → ids.
        let name_to_id: HashMap<String, NodeId> =
            crates.iter().map(|(id, n)| (n.clone(), *id)).collect();
        let mut efferent_set: HashMap<NodeId, BTreeSet<String>> = HashMap::new();
        let mut afferent_set: HashMap<NodeId, BTreeSet<String>> = HashMap::new();
        for edge in &edges {
            if let Some(consumer_id) = name_to_id.get(&edge.consumer_crate) {
                efferent_set
                    .entry(*consumer_id)
                    .or_default()
                    .insert(edge.producer_crate.clone());
            }
            if let Some(producer_id) = name_to_id.get(&edge.producer_crate) {
                afferent_set
                    .entry(*producer_id)
                    .or_default()
                    .insert(edge.consumer_crate.clone());
            }
        }

        let mut out: Vec<CrateMetric> = Vec::with_capacity(crates.len());
        for (crate_id, crate_name) in crates {
            let efferent = efferent_set
                .get(&crate_id)
                .map(|s| s.len())
                .unwrap_or(0) as u32;
            let afferent = afferent_set
                .get(&crate_id)
                .map(|s| s.len())
                .unwrap_or(0) as u32;
            let instability = if efferent + afferent == 0 {
                0.0
            } else {
                efferent as f64 / (efferent + afferent) as f64
            };
            let (total_items, trait_count, pub_alias_count) =
                item_counts.get(&crate_id).copied().unwrap_or((0, 0, 0));
            let abstractness = if total_items == 0 {
                0.0
            } else {
                (trait_count + pub_alias_count) as f64 / total_items as f64
            };
            out.push(CrateMetric {
                crate_id,
                crate_name,
                efferent,
                afferent,
                instability,
                abstractness,
                item_count: total_items,
            });
        }
        out.sort_by(|a, b| a.crate_name.cmp(&b.crate_name));
        Ok(out)
    }

    /// All cross-crate consumer→producer edges, decorated with the symbols
    /// carrying each edge. Cost is O(N_nodes + N_bindings + N_usages) — a
    /// single read transaction with three sequential scans.
    ///
    /// Note: local inherent method calls and local trait-declaration dispatch
    /// are captured as Method items in `total_refs_via_usages`. Remaining
    /// blind spots are indirect calls RA cannot resolve to a workspace Item
    /// (for example `dyn Trait` over external traits or generic `F: Fn(..)`).
    pub fn crate_edges(&self) -> Result<Vec<CrateEdge>> {
        let rtxn = self.env.read_txn()?;

        // Build crate index: every node → its crate id; every crate id → name.
        let mut node_to_crate: HashMap<NodeId, NodeId> = HashMap::new();
        let mut crate_name: HashMap<NodeId, String> = HashMap::new();
        let mut node_qual: HashMap<NodeId, String> = HashMap::new();
        let mut node_kind_label_map: HashMap<NodeId, String> = HashMap::new();
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            let mut id = [0u8; 32];
            id.copy_from_slice(key);
            let nid = NodeId(id);
            if node.kind == NodeKind::Crate {
                crate_name.insert(nid, node.qualified_name.clone());
            }
            if let Some(cid) = node.crate_id {
                node_to_crate.insert(nid, cid);
            }
            node_qual.insert(nid, node.qualified_name.clone());
            node_kind_label_map.insert(nid, node_kind_label(&node, label_item_kind));
        }

        // (consumer_crate, producer_crate, target, binding_kind) → import_count
        let mut import_acc: HashMap<(NodeId, NodeId, NodeId, BindingKind), usize> = HashMap::new();
        for entry in self.dbs.bindings_by_id.iter(&rtxn)? {
            let (_k, binding) = entry?;
            if binding.kind == BindingKind::Declared {
                continue;
            }
            let Some(consumer_crate) = node_to_crate.get(&binding.from_module).copied() else {
                continue;
            };
            let Some(producer_crate) = node_to_crate.get(&binding.target).copied() else {
                continue;
            };
            if consumer_crate == producer_crate {
                continue;
            }
            *import_acc
                .entry((consumer_crate, producer_crate, binding.target, binding.kind))
                .or_insert(0) += 1;
        }

        // (consumer_crate, producer_crate, target) → usage_count
        let mut usage_acc: HashMap<(NodeId, NodeId, NodeId), usize> = HashMap::new();
        for entry in self.dbs.usages_by_id.iter(&rtxn)? {
            let (_k, usage) = entry?;
            let Some(consumer_crate) = node_to_crate.get(&usage.consumer_module).copied() else {
                continue;
            };
            let Some(producer_crate) = node_to_crate.get(&usage.target).copied() else {
                continue;
            };
            if consumer_crate == producer_crate {
                continue;
            }
            *usage_acc
                .entry((consumer_crate, producer_crate, usage.target))
                .or_insert(0) += 1;
        }

        // Merge into per-edge per-symbol records.
        // Key: (consumer, producer) → Map<(target, binding_kind_label), (import_count, usage_count)>
        let mut per_edge: HashMap<(NodeId, NodeId), HashMap<(NodeId, Option<String>), (usize, usize)>> =
            HashMap::new();

        for ((c, p, t, bk), n) in import_acc {
            let entry = per_edge
                .entry((c, p))
                .or_default()
                .entry((t, Some(label_binding_kind(bk).to_string())))
                .or_insert((0, 0));
            entry.0 += n;
        }
        for ((c, p, t), n) in usage_acc {
            let entry = per_edge
                .entry((c, p))
                .or_default()
                // No binding_kind on a pure usage edge.
                .entry((t, None))
                .or_insert((0, 0));
            entry.1 += n;
        }

        let mut edges: Vec<CrateEdge> = Vec::new();
        for ((c, p), per_symbol) in per_edge {
            let consumer_crate = crate_name.get(&c).cloned().unwrap_or_default();
            let producer_crate = crate_name.get(&p).cloned().unwrap_or_default();

            // Collapse two rows for the same target (one with binding_kind,
            // one without) into one symbol row when possible. We keep the
            // binding_kind label if any binding exists for that target.
            let mut by_target: BTreeMap<NodeId, EdgeSymbol> = BTreeMap::new();
            for ((t, bk), (ic, uc)) in per_symbol {
                let target_qualified = node_qual.get(&t).cloned().unwrap_or_default();
                let target_kind = node_kind_label_map.get(&t).cloned().unwrap_or_default();
                let sym = by_target.entry(t).or_insert_with(|| EdgeSymbol {
                    target_qualified,
                    target_kind,
                    binding_kind: None,
                    import_count: 0,
                    usage_count: 0,
                });
                sym.import_count += ic;
                sym.usage_count += uc;
                if bk.is_some() && sym.binding_kind.is_none() {
                    sym.binding_kind = bk;
                }
            }

            let mut symbols: Vec<EdgeSymbol> = by_target.into_values().collect();
            symbols.sort_by(|a, b| {
                let ta = a.import_count + a.usage_count;
                let tb = b.import_count + b.usage_count;
                tb.cmp(&ta).then_with(|| a.target_qualified.cmp(&b.target_qualified))
            });

            let unique_symbols = symbols.len();
            let total_refs_via_imports = symbols.iter().map(|s| s.import_count).sum();
            let total_refs_via_usages = symbols.iter().map(|s| s.usage_count).sum();

            edges.push(CrateEdge {
                consumer_crate,
                producer_crate,
                unique_symbols,
                total_refs_via_imports,
                total_refs_via_usages,
                symbols,
            });
        }
        edges.sort_by(|a, b| {
            a.consumer_crate
                .cmp(&b.consumer_crate)
                .then_with(|| a.producer_crate.cmp(&b.producer_crate))
        });
        Ok(edges)
    }

    /// Pure filter over `crate_edges`. For every (consumer, producer) edge,
    /// test each rule; emit a violation when the consumer matches
    /// `rule.consumer`, the producer matches `rule.producer`, the consumer
    /// target kind is allowed by `rule.consumer_kinds`, and (if `except` is
    /// set) the consumer does NOT match `rule.except`.
    ///
    /// Patterns are glob-style with `*` wildcards. Pattern matching is on crate
    /// names; `consumer_kinds` defaults to `["lib", "bin"]`. Severity and
    /// message pass through to violations unchanged for caller-side rendering.
    pub fn forbidden_dependency_check(
        &self,
        rules: &[ForbiddenDependencyRule],
    ) -> Result<Vec<ForbiddenDependencyViolation>> {
        let edges = self.crate_edges()?;
        let crate_target_kind_by_name = self.crate_target_kind_by_name()?;
        let mut violations: Vec<ForbiddenDependencyViolation> = Vec::new();
        for edge in &edges {
            let consumer_kind = crate_target_kind_by_name
                .get(&edge.consumer_crate)
                .map(String::as_str)
                .unwrap_or("lib");
            for (idx, rule) in rules.iter().enumerate() {
                if !rule_allows_consumer_kind(rule, consumer_kind) {
                    continue;
                }
                if !glob_match(&rule.consumer, &edge.consumer_crate) {
                    continue;
                }
                if !glob_match(&rule.producer, &edge.producer_crate) {
                    continue;
                }
                if let Some(except) = rule.except.as_ref() {
                    if glob_match(except, &edge.consumer_crate) {
                        continue;
                    }
                }
                let total_refs = edge.total_refs_via_imports + edge.total_refs_via_usages;
                let sample_symbol = edge
                    .symbols
                    .first()
                    .map(|s| s.target_qualified.clone());
                violations.push(ForbiddenDependencyViolation {
                    rule_index: idx,
                    consumer_crate: edge.consumer_crate.clone(),
                    producer_crate: edge.producer_crate.clone(),
                    severity: rule.severity.clone(),
                    message: rule.message.clone(),
                    sample_symbol,
                    unique_symbols: edge.unique_symbols,
                    total_refs,
                });
            }
        }
        violations.sort_by(|a, b| {
            a.rule_index
                .cmp(&b.rule_index)
                .then_with(|| a.consumer_crate.cmp(&b.consumer_crate))
                .then_with(|| a.producer_crate.cmp(&b.producer_crate))
        });
        Ok(violations)
    }

    fn crate_target_kind_by_name(&self) -> Result<HashMap<String, String>> {
        let rtxn = self.read_txn()?;
        let mut out = HashMap::new();
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (_key, node) = entry?;
            if node.kind == NodeKind::Crate {
                out.insert(
                    node.qualified_name,
                    node.crate_target_kind.unwrap_or_else(|| "lib".to_string()),
                );
            }
        }
        Ok(out)
    }
}

/// Glob matcher with `*` wildcards (matches any run of chars, including
/// empty). No other metacharacters; pattern segments are matched as literal
/// substrings between wildcards. Greedy / linear in `text.len() *
/// pattern.len()`. Used by `forbidden_dependency_check`.
fn glob_match(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == text;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut cursor: usize = 0;
    let last = parts.len() - 1;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            // `*` at the boundary — anchors the next match anywhere from cursor.
            if i == last {
                return true;
            }
            continue;
        }
        if i == 0 {
            // No leading `*`: the first segment must anchor at start.
            if !text[cursor..].starts_with(part) {
                return false;
            }
            cursor += part.len();
        } else if i == last {
            // No trailing `*`: the last segment must anchor at end.
            return text[cursor..].ends_with(part)
                && text.len() - cursor >= part.len();
        } else {
            match text[cursor..].find(part) {
                Some(pos) => cursor += pos + part.len(),
                None => return false,
            }
        }
    }
    true
}

fn rule_allows_consumer_kind(rule: &ForbiddenDependencyRule, consumer_kind: &str) -> bool {
    let allowed = match rule.consumer_kinds.as_ref().filter(|kinds| !kinds.is_empty()) {
        Some(kinds) => kinds.as_slice(),
        None => return matches_default_consumer_kind(consumer_kind),
    };
    allowed.iter().any(|kind| {
        let kind = normalize_consumer_kind(kind);
        kind == "*" || kind == consumer_kind
    })
}

fn matches_default_consumer_kind(consumer_kind: &str) -> bool {
    matches!(consumer_kind, "lib" | "bin")
}

fn normalize_consumer_kind(kind: &str) -> String {
    match kind.trim() {
        "custom-build" => "build".to_string(),
        other => other.to_ascii_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity for the hand-rolled glob matcher.
    #[test]
    fn forbidden_glob_match_smoke() {
        assert!(glob_match("tokio", "tokio"));
        assert!(!glob_match("tokio", "tokio_util"));
        assert!(glob_match("*", ""));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("domain*", "domain_core"));
        assert!(glob_match("domain*", "domain"));
        assert!(!glob_match("domain*", "core_domain"));
        assert!(glob_match("*core", "domain_core"));
        assert!(!glob_match("*core", "domain_core_v2"));
        assert!(glob_match("foo*bar", "foobar"));
        assert!(glob_match("foo*bar", "foo_x_bar"));
        assert!(!glob_match("foo*bar", "foo"));
    }

    #[test]
    fn forbidden_dependency_rule_defaults_to_lib_and_bin_consumers() {
        let rule = ForbiddenDependencyRule {
            consumer: "*".into(),
            producer: "*".into(),
            consumer_kinds: None,
            except: None,
            severity: None,
            message: None,
        };

        assert!(rule_allows_consumer_kind(&rule, "lib"));
        assert!(rule_allows_consumer_kind(&rule, "bin"));
        assert!(!rule_allows_consumer_kind(&rule, "example"));
        assert!(!rule_allows_consumer_kind(&rule, "test"));
        assert!(!rule_allows_consumer_kind(&rule, "bench"));
        assert!(!rule_allows_consumer_kind(&rule, "build"));
    }

    #[test]
    fn forbidden_dependency_rule_explicit_consumer_kinds_override_default() {
        let rule = ForbiddenDependencyRule {
            consumer: "*".into(),
            producer: "*".into(),
            consumer_kinds: Some(vec!["example".into(), "custom-build".into()]),
            except: None,
            severity: None,
            message: None,
        };

        assert!(!rule_allows_consumer_kind(&rule, "lib"));
        assert!(rule_allows_consumer_kind(&rule, "example"));
        assert!(rule_allows_consumer_kind(&rule, "build"));
    }
}
