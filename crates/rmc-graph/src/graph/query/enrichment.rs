use super::super::labels::{
    binding_kind_label, item_kind_display_label, item_kind_short_label, node_kind_label,
    usage_category_label,
};
use super::super::model::{Binding, BindingVisibility, Namespace, Usage};
use super::super::snapshot::OpenedSnapshot;
use super::model::{
    CrateDeadPub, DeadPubFinding, EnrichedBinding, EnrichedCrateDeadPub, EnrichedDeadPub,
    EnrichedUsage,
};

impl OpenedSnapshot {
    pub fn enrich_bindings(&self, bindings: Vec<Binding>) -> Vec<EnrichedBinding> {
        let rtxn = match self.read_txn() {
            Ok(t) => t,
            Err(_) => return Vec::new(),
        };
        bindings
            .into_iter()
            .map(|binding| {
                let target_node = self.node_by_id(&rtxn, binding.target).ok().flatten();
                let from_module_node = self
                    .node_by_id(&rtxn, binding.from_module)
                    .ok()
                    .flatten();
                EnrichedBinding {
                    visible_name: binding.visible_name,
                    namespace: namespace_label(binding.namespace),
                    kind: binding_kind_label(binding.kind),
                    visibility: visibility_label(self, &rtxn, &binding.visibility),
                    from_module: from_module_node
                        .as_ref()
                        .map(|node| node.qualified_name.clone()),
                    target: target_node
                        .as_ref()
                        .map(|node| node.qualified_name.clone()),
                    target_kind: target_node
                        .as_ref()
                        .map(|node| node_kind_label(node, item_kind_short_label)),
                }
            })
            .collect()
    }

    pub fn enrich_usages(&self, usages: Vec<Usage>, summary: bool) -> Vec<EnrichedUsage> {
        let rtxn = match self.read_txn() {
            Ok(t) => t,
            Err(_) => return Vec::new(),
        };
        usages
            .into_iter()
            .map(|usage| {
                let consumer_node = self
                    .node_by_id(&rtxn, usage.consumer_module)
                    .ok()
                    .flatten();
                let consumer_function = usage.consumer_function.and_then(|fn_id| {
                    self.node_by_id(&rtxn, fn_id)
                        .ok()
                        .flatten()
                        .map(|node| node.qualified_name)
                });
                EnrichedUsage {
                    file: if summary { None } else { Some(usage.file) },
                    start: if summary { None } else { Some(usage.start) },
                    end: if summary { None } else { Some(usage.end) },
                    category: usage_category_label(usage.category),
                    consumer_module: consumer_node
                        .as_ref()
                        .map(|node| node.qualified_name.clone()),
                    consumer_function,
                }
            })
            .collect()
    }

    pub fn enrich_dead_pub(&self, finding: DeadPubFinding) -> EnrichedDeadPub {
        let rtxn = self.read_txn().ok();
        let visibility = match &rtxn {
            Some(txn) => visibility_label(self, txn, &finding.declared_visibility),
            None => "?".to_string(),
        };
        let (file, span) = match &rtxn {
            Some(txn) => match self.node_by_id(txn, finding.target).ok().flatten() {
                Some(node) => (node.file, node.span),
                None => (None, None),
            },
            None => (None, None),
        };
        EnrichedDeadPub {
            qualified_name: finding.qualified_name,
            item_kind: item_kind_display_label(finding.item_kind),
            declared_visibility: visibility,
            file,
            span,
        }
    }

    pub fn enrich_crate_dead_pub(&self, crate_report: CrateDeadPub) -> EnrichedCrateDeadPub {
        EnrichedCrateDeadPub {
            krate: crate_report.crate_qualified_name,
            findings: crate_report
                .findings
                .into_iter()
                .map(|finding| self.enrich_dead_pub(finding))
                .collect(),
        }
    }
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
) -> String {
    match visibility {
        BindingVisibility::Public => "pub".to_string(),
        BindingVisibility::Private => "private".to_string(),
        BindingVisibility::Crate(id) => match snap.node_by_id(rtxn, *id).ok().flatten() {
            Some(node) => format!("pub(crate={})", node.qualified_name),
            None => "pub(crate)".to_string(),
        },
        BindingVisibility::RestrictedTo(id) => match snap.node_by_id(rtxn, *id).ok().flatten() {
            Some(node) => format!("pub(in {})", node.qualified_name),
            None => "pub(in ?)".to_string(),
        },
    }
}
