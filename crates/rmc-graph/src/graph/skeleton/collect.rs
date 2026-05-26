use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Context, Result, bail};

use crate::graph::ids::NodeId;
use crate::graph::model::{ItemKind, Node, NodeKind};
use crate::graph::query::model::OverlapScope;
use crate::graph::query::shared::{crate_scope_allows, declared_visibility_map};
use crate::graph::snapshot::OpenedSnapshot;

use super::model::{
    CollectedSkeleton, SkeletonDiagnostic, SkeletonItem, SkeletonOptions, SkeletonSourceFile,
};

pub(super) fn collect_skeleton(
    snap: &OpenedSnapshot,
    opts: &SkeletonOptions,
) -> Result<CollectedSkeleton> {
    let include = IncludeFilter::parse(&opts.include)?;
    let rtxn = snap.read_txn()?;
    let mut nodes: HashMap<NodeId, Node> = HashMap::new();
    let mut crate_names: HashMap<NodeId, String> = HashMap::new();
    let mut crate_target_kind_for: HashMap<NodeId, String> = HashMap::new();
    let mut vendor_crates: HashSet<NodeId> = HashSet::new();

    for entry in snap.dbs.nodes_by_id.iter(&rtxn)? {
        let (key, node) = entry?;
        let mut id = [0u8; 32];
        id.copy_from_slice(key);
        let id = NodeId(id);
        if node.kind == NodeKind::Crate {
            crate_names.insert(id, node.qualified_name.clone());
            crate_target_kind_for.insert(
                id,
                node.crate_target_kind
                    .clone()
                    .unwrap_or_else(|| "lib".to_string()),
            );
        }
        if let (Some(crate_id), Some(file)) = (node.crate_id, node.file.as_deref()) {
            if file.starts_with("vendor/") {
                vendor_crates.insert(crate_id);
            }
        }
        nodes.insert(id, node);
    }

    let selected_crates = selected_crates(
        opts,
        &nodes,
        &crate_target_kind_for,
        &vendor_crates,
    );
    let mut diagnostics = Vec::new();
    if selected_crates.is_empty() {
        diagnostics.push(SkeletonDiagnostic {
            message: "no local lib/bin crates matched skeleton filters".to_string(),
        });
    }

    let target_ids: HashSet<NodeId> = nodes
        .iter()
        .filter_map(|(id, node)| {
            let crate_id = node.crate_id?;
            if !selected_crates.contains(&crate_id) {
                return None;
            }
            if matches!(node.kind, NodeKind::Item | NodeKind::Module) {
                Some(*id)
            } else {
                None
            }
        })
        .collect();
    let visibility = declared_visibility_map(snap, &rtxn, &target_ids)?;

    let mut retained_items: BTreeMap<NodeId, SkeletonItem> = BTreeMap::new();
    let mut retained_hosts: HashSet<NodeId> = HashSet::new();

    let mut selected: Vec<NodeId> = selected_crates.iter().copied().collect();
    selected.sort_by(|a, b| {
        crate_names
            .get(a)
            .cmp(&crate_names.get(b))
            .then_with(|| a.as_bytes().cmp(b.as_bytes()))
    });

    for crate_id in selected {
        let Some(root_id) = snap.find_root_module_of(crate_id)? else {
            diagnostics.push(SkeletonDiagnostic {
                message: format!(
                    "crate `{}` has no root module",
                    crate_names.get(&crate_id).cloned().unwrap_or_default()
                ),
            });
            continue;
        };
        let _ = walk_module(
            snap,
            &rtxn,
            &nodes,
            &visibility,
            &include,
            opts,
            root_id,
            true,
            &mut retained_items,
            &mut retained_hosts,
        )?;
    }

    if opts.include_impls {
        collect_inherent_assoc_items(
            &nodes,
            &visibility,
            &include,
            opts,
            &retained_hosts,
            &mut retained_items,
        );
    }

    let files = bucket_items(retained_items.into_values().collect(), &mut diagnostics);
    Ok(CollectedSkeleton { files, diagnostics })
}

#[allow(clippy::too_many_arguments)]
fn walk_module(
    snap: &OpenedSnapshot,
    rtxn: &heed::RoTxn<'_, heed::WithoutTls>,
    nodes: &HashMap<NodeId, Node>,
    visibility: &HashMap<NodeId, String>,
    include: &IncludeFilter,
    opts: &SkeletonOptions,
    module_id: NodeId,
    is_root: bool,
    retained_items: &mut BTreeMap<NodeId, SkeletonItem>,
    retained_hosts: &mut HashSet<NodeId>,
) -> Result<bool> {
    let node = nodes
        .get(&module_id)
        .with_context(|| "dangling module id in skeleton walk")?;
    if opts.skip_test_items && node.qualified_name.contains("::tests::") {
        return Ok(false);
    }
    let module_visibility = visibility.get(&module_id).cloned();
    if !is_root && !include.allows(module_visibility.as_deref()) {
        return Ok(false);
    }

    let mut child_ids = Vec::new();
    if let Some(iter) = snap
        .dbs
        .children_by_parent
        .get_duplicates(rtxn, module_id.as_bytes())?
    {
        for entry in iter {
            let (_k, child_bytes) = entry?;
            let mut id = [0u8; 32];
            id.copy_from_slice(child_bytes);
            child_ids.push(NodeId(id));
        }
    }
    child_ids.sort_by(|a, b| {
        let a_node = nodes.get(a);
        let b_node = nodes.get(b);
        a_node
            .and_then(|n| n.span.map(|span| span.0))
            .cmp(&b_node.and_then(|n| n.span.map(|span| span.0)))
            .then_with(|| {
                a_node
                    .map(|n| n.qualified_name.as_str())
                    .cmp(&b_node.map(|n| n.qualified_name.as_str()))
            })
            .then_with(|| a.as_bytes().cmp(b.as_bytes()))
    });

    let mut kept_modules = 0;
    let mut kept_items = 0;
    for child_id in child_ids {
        let Some(child) = nodes.get(&child_id) else {
            continue;
        };
        match child.kind {
            NodeKind::Module => {
                if walk_module(
                    snap,
                    rtxn,
                    nodes,
                    visibility,
                    include,
                    opts,
                    child_id,
                    false,
                    retained_items,
                    retained_hosts,
                )? {
                    kept_modules += 1;
                }
            }
            NodeKind::Item => {
                if should_skip_direct_item(child) {
                    continue;
                }
                if let Some(item) = retain_item(child_id, child, visibility, include, opts) {
                    if is_assoc_host(child) {
                        retained_hosts.insert(child_id);
                    }
                    retained_items.insert(child_id, item);
                    kept_items += 1;
                }
            }
            _ => {}
        }
    }

    if !is_root && kept_modules == 0 && kept_items == 0 {
        return Ok(false);
    }

    Ok(true)
}

fn selected_crates(
    opts: &SkeletonOptions,
    nodes: &HashMap<NodeId, Node>,
    crate_target_kind_for: &HashMap<NodeId, String>,
    vendor_crates: &HashSet<NodeId>,
) -> HashSet<NodeId> {
    let scope = if opts.exclude_vendor {
        OverlapScope::LocalNoVendor
    } else {
        OverlapScope::Local
    };
    nodes
        .iter()
        .filter_map(|(id, node)| {
            if node.kind != NodeKind::Crate {
                return None;
            }
            if !crate_scope_allows(scope, *id, crate_target_kind_for, vendor_crates) {
                return None;
            }
            if let Some(filters) = &opts.crates {
                let matches_filter = filters.iter().any(|filter| {
                    filter == &node.qualified_name || filter == &node.display_name
                });
                if !matches_filter {
                    return None;
                }
            }
            Some(*id)
        })
        .collect()
}

fn retain_item(
    id: NodeId,
    node: &Node,
    visibility: &HashMap<NodeId, String>,
    include: &IncludeFilter,
    opts: &SkeletonOptions,
) -> Option<SkeletonItem> {
    if opts.skip_test_items && is_test_item(node) {
        return None;
    }
    let item_visibility = visibility.get(&id).cloned();
    if !include.allows(item_visibility.as_deref()) {
        return None;
    }
    Some(SkeletonItem {
        id,
        node: node.clone(),
        parent: None,
        visibility: item_visibility,
    })
}

fn collect_inherent_assoc_items(
    nodes: &HashMap<NodeId, Node>,
    visibility: &HashMap<NodeId, String>,
    include: &IncludeFilter,
    opts: &SkeletonOptions,
    retained_hosts: &HashSet<NodeId>,
    retained_items: &mut BTreeMap<NodeId, SkeletonItem>,
) {
    for (id, node) in nodes {
        let Some(parent_id) = node.parent_id else {
            continue;
        };
        if !retained_hosts.contains(&parent_id) {
            continue;
        }
        if !matches!(
            node.item_kind,
            Some(ItemKind::Method | ItemKind::AssocConst | ItemKind::AssocType)
        ) {
            continue;
        }
        if let Some(mut item) = retain_item(*id, node, visibility, include, opts) {
            item.parent = nodes.get(&parent_id).cloned();
            retained_items.insert(*id, item);
        }
    }
}

fn bucket_items(
    items: Vec<SkeletonItem>,
    diagnostics: &mut Vec<SkeletonDiagnostic>,
) -> Vec<SkeletonSourceFile> {
    let mut by_file: BTreeMap<String, Vec<SkeletonItem>> = BTreeMap::new();
    let mut crate_by_file: HashMap<String, String> = HashMap::new();
    for item in items {
        let Some(source_path) = item.node.file.clone() else {
            diagnostics.push(SkeletonDiagnostic {
                message: format!(
                    "item `{}` has no source file and was not mirrored",
                    item.node.qualified_name,
                ),
            });
            continue;
        };
        let crate_name = item
            .node
            .qualified_name
            .split("::")
            .next()
            .unwrap_or_default()
            .to_string();
        crate_by_file
            .entry(source_path.clone())
            .or_insert(crate_name);
        by_file.entry(source_path).or_default().push(item);
    }

    by_file
        .into_iter()
        .map(|(source_path, mut items)| {
            items.sort_by(|a, b| {
                a.node
                    .span
                    .map(|span| span.0)
                    .cmp(&b.node.span.map(|span| span.0))
                    .then_with(|| a.node.qualified_name.cmp(&b.node.qualified_name))
                    .then_with(|| a.id.as_bytes().cmp(b.id.as_bytes()))
            });
            SkeletonSourceFile {
                crate_name: crate_by_file.remove(&source_path).unwrap_or_default(),
                skeleton_path: format!(".skeleton/{source_path}"),
                source_path,
                items,
            }
        })
        .collect()
}

fn should_skip_direct_item(node: &Node) -> bool {
    matches!(node.item_kind, Some(ItemKind::EnumVariant))
        || matches!(
            node.item_kind,
            Some(ItemKind::Method | ItemKind::AssocConst | ItemKind::AssocType)
        )
}

fn is_assoc_host(node: &Node) -> bool {
    matches!(
        node.item_kind,
        Some(ItemKind::Struct | ItemKind::Enum | ItemKind::Union)
    )
}

fn is_test_item(node: &Node) -> bool {
    if node.qualified_name.contains("::tests::") {
        return true;
    }
    node.attributes.iter().any(|attr| {
        let compact: String = attr.chars().filter(|c| !c.is_whitespace()).collect();
        compact.starts_with("#[test")
            || compact.contains("cfg(test)")
            || compact.contains("cfg_attr(test")
    })
}

#[derive(Debug, Clone)]
struct IncludeFilter {
    all: bool,
    buckets: HashSet<&'static str>,
}

impl IncludeFilter {
    fn parse(raw: &[String]) -> Result<Self> {
        let values: Vec<&str> = if raw.is_empty() {
            vec!["pub", "pub(crate)"]
        } else {
            raw.iter().map(String::as_str).collect()
        };
        let mut buckets = HashSet::new();
        let mut all = false;
        for value in values {
            match value {
                "all" => all = true,
                "pub" => {
                    buckets.insert("pub");
                }
                "pub(crate)" => {
                    buckets.insert("pub(crate)");
                }
                "restricted" => {
                    buckets.insert("restricted");
                }
                "private" => {
                    buckets.insert("private");
                }
                other => bail!(
                    "unknown skeleton include value `{other}`; expected pub | pub(crate) | restricted | private | all"
                ),
            }
        }
        Ok(Self { all, buckets })
    }

    fn allows(&self, visibility: Option<&str>) -> bool {
        if self.all {
            return true;
        }
        match visibility_bucket(visibility) {
            "pub" => self.buckets.contains("pub"),
            "pub(crate)" => self.buckets.contains("pub(crate)"),
            "restricted" => self.buckets.contains("restricted"),
            _ => self.buckets.contains("private"),
        }
    }
}

fn visibility_bucket(visibility: Option<&str>) -> &'static str {
    match visibility {
        Some("pub") => "pub",
        Some("pub(crate)") => "pub(crate)",
        Some("pub(self)") | None => "private",
        Some(vis) if vis.starts_with("pub(in ") => "restricted",
        Some(_) => "private",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::test_support::shared_snapshot;

    fn render(opts: SkeletonOptions) -> CollectedSkeleton {
        collect_skeleton(shared_snapshot(), &opts).expect("collect skeleton")
    }

    #[test]
    fn include_all_keeps_more_items_than_default() {
        let default = render(SkeletonOptions::default());
        let all = render(SkeletonOptions {
            include: vec!["all".to_string()],
            ..Default::default()
        });
        let default_items: usize = default.files.iter().map(|file| file.items.len()).sum();
        let all_items: usize = all.files.iter().map(|file| file.items.len()).sum();
        assert!(all_items >= default_items);
        assert!(all_items > 0);
    }

    #[test]
    fn skip_test_items_prunes_tests_modules_and_test_attrs() {
        let skipped = render(SkeletonOptions {
            include: vec!["all".to_string()],
            skip_test_items: true,
            ..Default::default()
        });
        let kept = render(SkeletonOptions {
            include: vec!["all".to_string()],
            skip_test_items: false,
            ..Default::default()
        });
        let skipped_names: Vec<&str> = skipped
            .files
            .iter()
            .flat_map(|file| file.items.iter())
            .map(|item| item.node.qualified_name.as_str())
            .collect();
        let kept_names: Vec<&str> = kept
            .files
            .iter()
            .flat_map(|file| file.items.iter())
            .map(|item| item.node.qualified_name.as_str())
            .collect();
        assert!(skipped_names.iter().all(|name| !name.contains("::tests::")));
        assert!(kept_names.iter().any(|name| name.contains("::tests::")));
    }

    #[test]
    fn collected_files_and_items_are_deterministically_ordered() {
        let collected = render(SkeletonOptions {
            include: vec!["all".to_string()],
            ..Default::default()
        });
        let paths: Vec<&str> = collected
            .files
            .iter()
            .map(|file| file.source_path.as_str())
            .collect();
        let mut sorted_paths = paths.clone();
        sorted_paths.sort();
        assert_eq!(paths, sorted_paths);

        for file in collected.files {
            let keys: Vec<(u32, &str)> = file
                .items
                .iter()
                .map(|item| {
                    (
                        item.node.span.map(|span| span.0).unwrap_or(u32::MAX),
                        item.node.qualified_name.as_str(),
                    )
                })
                .collect();
            let mut sorted_keys = keys.clone();
            sorted_keys.sort();
            assert_eq!(
                keys, sorted_keys,
                "items not sorted in {}",
                file.source_path,
            );
        }
    }
}
