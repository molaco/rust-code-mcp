//! Query methods on `OpenedSnapshot` — overlaps family.
//!
//! Covers cross-workspace overlap detection: `overlaps`,
//! `overlaps_with_scope`. Moved here from `graph::queries` in PR 11.

use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::Result;

use super::super::ids::NodeId;
use super::super::labels::item_kind_short_label as label_item_kind;
use super::super::model::{ItemKind, Node, NodeKind};
use super::super::snapshot::OpenedSnapshot;
use super::model::{
    CommonFnName, ModuleShadow, OverlapScope, OverlapsReport, TypeCollision, TypeLocation,
    WithinCrateDuplicate,
};
use super::shared::crate_scope_allows;

impl OpenedSnapshot {
    /// Single-pass over `nodes_by_id`. Detects cross-crate type collisions,
    /// module shadowing of crate names, within-crate type duplicates, and
    /// fn names that appear in 4+ crates.
    pub fn overlaps(&self) -> Result<OverlapsReport> {
        self.overlaps_with_scope(OverlapScope::All)
    }

    pub fn overlaps_with_scope(&self, scope: OverlapScope) -> Result<OverlapsReport> {
        let rtxn = self.env.read_txn()?;

        let mut crate_name_for: HashMap<NodeId, String> = HashMap::new();
        let mut crate_target_kind_for: HashMap<NodeId, String> = HashMap::new();
        let mut vendor_crates: HashSet<NodeId> = HashSet::new();

        // First pass: build crate indexes and detect crates whose local
        // source lives under vendor/.
        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            if node.kind == NodeKind::Crate {
                let mut id = [0u8; 32];
                id.copy_from_slice(key);
                let crate_id = NodeId(id);
                crate_name_for.insert(crate_id, node.display_name.clone());
                crate_target_kind_for.insert(
                    crate_id,
                    node.crate_target_kind.unwrap_or_else(|| "lib".to_string()),
                );
            }
            if let (Some(crate_id), Some(file)) = (node.crate_id, node.file.as_deref()) {
                if file.starts_with("vendor/") {
                    vendor_crates.insert(crate_id);
                }
            }
        }
        let allowed_crates: HashSet<NodeId> = crate_name_for
            .keys()
            .copied()
            .filter(|crate_id| {
                crate_scope_allows(
                    scope,
                    *crate_id,
                    &crate_target_kind_for,
                    &vendor_crates,
                )
            })
            .collect();
        let crate_names: HashSet<String> = allowed_crates
            .iter()
            .filter_map(|crate_id| crate_name_for.get(crate_id).cloned())
            .collect();

        // Group containers we'll fill on the second pass.
        let mut type_groups: HashMap<String, Vec<(NodeId, Node, NodeId)>> = HashMap::new();
        let mut shadows: Vec<ModuleShadow> = Vec::new();
        let mut within_crate_types: HashMap<(NodeId, String), Vec<Node>> = HashMap::new();
        let mut fn_spread: HashMap<String, BTreeSet<String>> = HashMap::new();

        for entry in self.dbs.nodes_by_id.iter(&rtxn)? {
            let (key, node) = entry?;
            let mut id = [0u8; 32];
            id.copy_from_slice(key);
            let nid = NodeId(id);

            if node.kind == NodeKind::Module {
                if let Some(crate_id) = node.crate_id {
                    if !allowed_crates.contains(&crate_id) {
                        continue;
                    }
                    let owning_crate = crate_name_for.get(&crate_id).cloned().unwrap_or_default();
                    if crate_names.contains(&node.display_name)
                        && node.display_name != owning_crate
                    {
                        shadows.push(ModuleShadow {
                            crate_name: owning_crate,
                            module_qualified: node.qualified_name.clone(),
                            shadowed_crate: node.display_name.clone(),
                        });
                    }
                }
            }

            if node.kind != NodeKind::Item {
                continue;
            }
            let Some(item_kind) = node.item_kind else {
                continue;
            };
            let Some(crate_id) = node.crate_id else {
                continue;
            };
            if !allowed_crates.contains(&crate_id) {
                continue;
            }

            // Type-kind items participate in collision and within-crate dup checks.
            if matches!(
                item_kind,
                ItemKind::Struct | ItemKind::Enum | ItemKind::Trait | ItemKind::TypeAlias
            ) {
                type_groups
                    .entry(node.display_name.clone())
                    .or_default()
                    .push((nid, node.clone(), crate_id));
                within_crate_types
                    .entry((crate_id, node.display_name.clone()))
                    .or_default()
                    .push(node.clone());
            }

            // Fn-spread check.
            if item_kind == ItemKind::Function {
                if let Some(crate_dn) = crate_name_for.get(&crate_id) {
                    fn_spread
                        .entry(node.display_name.clone())
                        .or_default()
                        .insert(crate_dn.clone());
                }
            }
        }

        // Cross-crate type collisions: name appears in ≥2 distinct crates.
        let mut cross_crate_type_collisions: Vec<TypeCollision> = type_groups
            .into_iter()
            .filter_map(|(name, group)| {
                let distinct: HashSet<NodeId> = group.iter().map(|(_, _, c)| *c).collect();
                if distinct.len() < 2 {
                    return None;
                }
                let mut locations: Vec<TypeLocation> = group
                    .into_iter()
                    .map(|(_, n, cid)| TypeLocation {
                        crate_name: crate_name_for.get(&cid).cloned().unwrap_or_default(),
                        qualified_name: n.qualified_name,
                        item_kind: n.item_kind.map(label_item_kind).unwrap_or("?").to_string(),
                    })
                    .collect();
                locations.sort_by(|a, b| {
                    a.crate_name
                        .cmp(&b.crate_name)
                        .then_with(|| a.qualified_name.cmp(&b.qualified_name))
                });
                Some(TypeCollision { name, locations })
            })
            .collect();
        cross_crate_type_collisions.sort_by(|a, b| a.name.cmp(&b.name));

        // Within-crate duplicates: ≥2 entries under the same (crate, name).
        let mut within_crate_type_duplicates: Vec<WithinCrateDuplicate> = within_crate_types
            .into_iter()
            .filter_map(|((cid, name), nodes)| {
                if nodes.len() < 2 {
                    return None;
                }
                let mut qualified_names: Vec<String> =
                    nodes.into_iter().map(|n| n.qualified_name).collect();
                qualified_names.sort();
                Some(WithinCrateDuplicate {
                    crate_name: crate_name_for.get(&cid).cloned().unwrap_or_default(),
                    name,
                    qualified_names,
                })
            })
            .collect();
        within_crate_type_duplicates.sort_by(|a, b| {
            a.crate_name
                .cmp(&b.crate_name)
                .then_with(|| a.name.cmp(&b.name))
        });

        let mut common_fn_names: Vec<CommonFnName> = fn_spread
            .into_iter()
            .filter(|(_, set)| set.len() >= 4)
            .map(|(name, set)| CommonFnName {
                name,
                crates: set.into_iter().collect(),
            })
            .collect();
        common_fn_names.sort_by(|a, b| {
            b.crates.len().cmp(&a.crates.len()).then_with(|| a.name.cmp(&b.name))
        });

        shadows.sort_by(|a, b| {
            a.crate_name
                .cmp(&b.crate_name)
                .then_with(|| a.module_qualified.cmp(&b.module_qualified))
        });

        Ok(OverlapsReport {
            cross_crate_type_collisions,
            module_shadows: shadows,
            within_crate_type_duplicates,
            common_fn_names,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::shared::crate_scope_allows;

    #[test]
    fn overlap_scope_filters_examples_and_vendor() {
        let lib_crate = NodeId([1u8; 32]);
        let example_crate = NodeId([2u8; 32]);
        let vendor_crate = NodeId([3u8; 32]);
        let mut target_kinds = HashMap::new();
        target_kinds.insert(lib_crate, "lib".to_string());
        target_kinds.insert(example_crate, "example".to_string());
        target_kinds.insert(vendor_crate, "lib".to_string());
        let vendor_crates = HashSet::from([vendor_crate]);

        assert!(crate_scope_allows(
            OverlapScope::All,
            example_crate,
            &target_kinds,
            &vendor_crates,
        ));
        assert!(crate_scope_allows(
            OverlapScope::Local,
            vendor_crate,
            &target_kinds,
            &vendor_crates,
        ));
        assert!(!crate_scope_allows(
            OverlapScope::Local,
            example_crate,
            &target_kinds,
            &vendor_crates,
        ));
        assert!(!crate_scope_allows(
            OverlapScope::LocalNoVendor,
            vendor_crate,
            &target_kinds,
            &vendor_crates,
        ));
        assert!(crate_scope_allows(
            OverlapScope::LocalNoVendor,
            lib_crate,
            &target_kinds,
            &vendor_crates,
        ));
    }
}
