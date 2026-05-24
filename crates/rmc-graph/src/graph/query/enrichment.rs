use anyhow::{Context, Result};

use super::super::labels::{
    binding_kind_label, item_kind_display_label, item_kind_short_label, node_kind_label,
    usage_category_label,
};
use super::super::ids::NodeId;
use super::super::model::{Binding, BindingVisibility, Namespace, Node, Usage};
use super::super::snapshot::OpenedSnapshot;
use super::model::{
    CrateDeadPub, DeadPubFinding, EnrichedBinding, EnrichedCrateDeadPub, EnrichedDeadPub,
    EnrichedUsage,
};

impl OpenedSnapshot {
    pub fn enrich_bindings(&self, bindings: Vec<Binding>) -> Result<Vec<EnrichedBinding>> {
        let rtxn = self
            .read_txn()
            .context("read graph snapshot transaction for binding enrichment")?;
        bindings
            .into_iter()
            .map(|binding| {
                let target_node = required_node_by_id(self, &rtxn, binding.target)?;
                let from_module_node = required_node_by_id(self, &rtxn, binding.from_module)?;
                Ok(EnrichedBinding {
                    visible_name: binding.visible_name,
                    namespace: namespace_label(binding.namespace),
                    kind: binding_kind_label(binding.kind),
                    visibility: visibility_label(self, &rtxn, &binding.visibility)?,
                    from_module: Some(from_module_node.qualified_name.clone()),
                    target: Some(target_node.qualified_name.clone()),
                    target_kind: Some(node_kind_label(&target_node, item_kind_short_label)),
                })
            })
            .collect()
    }

    pub fn enrich_usages(&self, usages: Vec<Usage>, summary: bool) -> Result<Vec<EnrichedUsage>> {
        let rtxn = self
            .read_txn()
            .context("read graph snapshot transaction for usage enrichment")?;
        usages
            .into_iter()
            .map(|usage| {
                let consumer_node = required_node_by_id(self, &rtxn, usage.consumer_module)?;
                let consumer_function = match usage.consumer_function {
                    Some(fn_id) => Some(required_node_by_id(self, &rtxn, fn_id)?.qualified_name),
                    None => None,
                };
                Ok(EnrichedUsage {
                    file: if summary { None } else { Some(usage.file) },
                    start: if summary { None } else { Some(usage.start) },
                    end: if summary { None } else { Some(usage.end) },
                    category: usage_category_label(usage.category),
                    consumer_module: Some(consumer_node.qualified_name.clone()),
                    consumer_function,
                })
            })
            .collect()
    }

    pub fn enrich_dead_pub(&self, finding: DeadPubFinding) -> Result<EnrichedDeadPub> {
        let rtxn = self
            .read_txn()
            .context("read graph snapshot transaction for dead-pub enrichment")?;
        let visibility = visibility_label(self, &rtxn, &finding.declared_visibility)?;
        let node = required_node_by_id(self, &rtxn, finding.target)?;
        Ok(EnrichedDeadPub {
            qualified_name: finding.qualified_name,
            item_kind: item_kind_display_label(finding.item_kind),
            declared_visibility: visibility,
            file: node.file,
            span: node.span,
        })
    }

    pub fn enrich_crate_dead_pub(&self, crate_report: CrateDeadPub) -> Result<EnrichedCrateDeadPub> {
        Ok(EnrichedCrateDeadPub {
            krate: crate_report.crate_qualified_name,
            findings: crate_report
                .findings
                .into_iter()
                .map(|finding| self.enrich_dead_pub(finding))
                .collect::<Result<Vec<_>>>()?,
        })
    }
}

fn required_node_by_id(
    snap: &OpenedSnapshot,
    rtxn: &heed::RoTxn<'_, heed::WithoutTls>,
    id: NodeId,
) -> Result<Node> {
    snap.node_by_id(rtxn, id)?
        .with_context(|| format!("graph node {} was referenced but not found", id.to_hex()))
}

fn namespace_label(namespace: Namespace) -> &'static str {
    match namespace {
        Namespace::Type => "Type",
        Namespace::Value => "Value",
    }
}

fn visibility_label(
    snap: &OpenedSnapshot,
    rtxn: &heed::RoTxn<'_, heed::WithoutTls>,
    visibility: &BindingVisibility,
) -> Result<String> {
    match visibility {
        BindingVisibility::Public => Ok("pub".to_string()),
        BindingVisibility::Private => Ok("private".to_string()),
        BindingVisibility::Crate(id) => {
            let node = required_node_by_id(snap, rtxn, *id)?;
            Ok(format!("pub(crate={})", node.qualified_name))
        }
        BindingVisibility::RestrictedTo(id) => {
            let node = required_node_by_id(snap, rtxn, *id)?;
            Ok(format!("pub(in {})", node.qualified_name))
        }
    }
}
