//! Public-surface endpoint family.
//!
//! Endpoints that interrogate the workspace's public API and its hygiene —
//! dead public items, enum variants, function signatures, attribute-tagged
//! items, re-export chains, doc/derive audits, and name-overlap reports.
//! Each endpoint follows the shape documented in `graph_tools.rs`: resolve
//! directory, open snapshot, resolve qualified names, run the query,
//! serialize.

use serde::Serialize;

use crate::graph::labels::item_kind_display_label as item_kind_label;
use crate::graph::queries::ItemWithAttribute;
use crate::graph::{
    CrateDeadPub, DeadPubFinding, FunctionFilter, FunctionSignature, FunctionWithSignature,
    ItemKind, Node, NodeId, NodeKind, OpenedSnapshot, OverlapsReport,
    PubTypeAliasMasqueradingAsReexport, ReExportChain, SelfKindFilter,
};
use crate::tools::graph::response::*;
use crate::tools::params::{
    DeadPubParams, DeadPubReportParams, EnumVariantsParams, FunctionSignatureParams,
    FunctionsWithFilterParams, ItemAttributesParams, ItemsWithAttributeParams, OverlapsParams,
    PubUsePubTypeAuditParams, ReExportChainParams,
};

use rmcp::{ErrorData as McpError, model::CallToolResult};

pub async fn dead_pub_in_crate(params: DeadPubParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;

    // Caller may pass a crate name (e.g. `my_crate`) or a crate root module name —
    // both resolve via `lookup_by_qualified_name`. Promote module → owning crate
    // if a Module came back so the rest of the function only handles Crate.
    let (id, node) = snap
        .lookup_by_qualified_name(&params.krate)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.krate),
                None,
            )
        })?;
    let crate_id = match node.kind {
        NodeKind::Crate => id,
        NodeKind::Module => node
            .crate_id
            .or(node.parent_id)
            .ok_or_else(|| {
                McpError::invalid_params(
                    format!("`{}` resolves to a Module with no crate_id", params.krate),
                    None,
                )
            })?,
        other => {
            return Err(McpError::invalid_params(
                format!(
                    "`{}` is a {other:?}, expected a crate or its root module",
                    params.krate
                ),
                None,
            ));
        }
    };

    let findings = snap
        .dead_pub_in_crate(crate_id)
        .map_err(internal_error("dead_pub_in_crate"))?;

    let page_req = list_page(&params.pagination);
    let mut enriched: Vec<EnrichedDeadPub> = findings
        .into_iter()
        .map(|f| enrich_dead_pub(&snap, f))
        .collect();
    clear_locations_for_summary(&mut enriched, page_req.summary, |finding| {
        finding.file = None;
        finding.span = None;
    });
    let (page, findings) = page_list(enriched, page_req);
    json_result(&DeadPubResponse {
        krate: params.krate,
        page,
        findings,
    })
}

pub async fn dead_pub_report(params: DeadPubReportParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let report = snap
        .dead_pub_report()
        .map_err(internal_error("dead_pub_report"))?;

    let crates: Vec<EnrichedCrateDeadPub> = report
        .into_iter()
        .map(|c| enrich_crate_dead_pub(&snap, c))
        .collect();
    let total: usize = crates.iter().map(|c| c.findings.len()).sum();
    let page_req = list_page(&params.pagination);
    let mut flat: Vec<(String, EnrichedDeadPub)> = Vec::new();
    for c in crates {
        for finding in c.findings {
            flat.push((c.krate.clone(), finding));
        }
    }
    clear_locations_for_summary(&mut flat, page_req.summary, |(_, finding)| {
        finding.file = None;
        finding.span = None;
    });
    let (page, flat) = page_list(flat, page_req);
    let mut crates: Vec<EnrichedCrateDeadPub> = Vec::new();
    for (krate, finding) in flat {
        if let Some(last) = crates.last_mut() {
            if last.krate == krate {
                last.findings.push(finding);
                continue;
            }
        }
        crates.push(EnrichedCrateDeadPub {
            krate,
            findings: vec![finding],
        });
    }
    json_result(&DeadPubReportResponse {
        workspace: params.directory,
        total_findings: total,
        page,
        crates,
    })
}

pub async fn enum_variants(params: EnumVariantsParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let (enum_id, enum_node) = snap
        .lookup_by_qualified_name(&params.target)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.target),
                None,
            )
        })?;
    if enum_node.item_kind != Some(ItemKind::Enum) {
        return Err(McpError::invalid_params(
            format!(
                "`{}` is not an Enum (got {:?}); enum_variants only enumerates enum variants",
                params.target, enum_node.item_kind
            ),
            None,
        ));
    }
    let variants: Vec<Node> = snap
        .enum_variants(enum_id)
        .map_err(internal_error("enum_variants"))?;

    let mut enriched: Vec<EnrichedEnumVariant> = variants
        .into_iter()
        .map(|n| EnrichedEnumVariant {
            display_name: n.display_name,
            qualified_name: n.qualified_name,
            file: n.file,
            span: n.span,
        })
        .collect();
    let page_req = list_page(&params.pagination);
    clear_locations_for_summary(&mut enriched, page_req.summary, |variant| {
        variant.file = None;
        variant.span = None;
    });
    let variant_count = enriched.len();
    let (page, variants) = page_list(enriched, page_req);
    json_result(&EnumVariantsResponse {
        enum_qualified_name: enum_node.qualified_name,
        variant_count,
        page,
        variants,
    })
}

pub async fn item_attributes(
    params: ItemAttributesParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let (target_id, target_node) = snap
        .lookup_by_qualified_name(&params.target)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.target),
                None,
            )
        })?;
    let attrs = snap
        .item_attributes(target_id)
        .map_err(internal_error("item_attributes"))?;
    let (page, attributes) = page_list(attrs, list_page(&params.pagination));
    json_result(&ItemAttributesResponse {
        target: target_node.qualified_name,
        item_kind: target_node.item_kind.map(item_kind_label),
        file: target_node.file,
        span: target_node.span,
        attribute_count: page.total_match_count,
        page,
        attributes,
    })
}

pub async fn items_with_attribute(
    params: ItemsWithAttributeParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let (id, node) = snap
        .lookup_by_qualified_name(&params.crate_name)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.crate_name),
                None,
            )
        })?;
    let crate_id = match node.kind {
        NodeKind::Crate => id,
        NodeKind::Module => node.crate_id.or(node.parent_id).ok_or_else(|| {
            McpError::invalid_params(
                format!(
                    "`{}` resolves to a Module with no crate_id",
                    params.crate_name
                ),
                None,
            )
        })?,
        other => {
            return Err(McpError::invalid_params(
                format!(
                    "`{}` is a {other:?}, expected a crate or its root module",
                    params.crate_name
                ),
                None,
            ));
        }
    };
    let hits: Vec<ItemWithAttribute> = snap
        .items_with_attribute(crate_id, &params.attribute_pattern)
        .map_err(internal_error("items_with_attribute"))?;
    let mut enriched: Vec<EnrichedItemWithAttribute> = hits
        .into_iter()
        .map(|h| EnrichedItemWithAttribute {
            qualified_name: h.qualified_name,
            item_kind: h.item_kind.map(item_kind_label),
            matched_attribute: h.matched_attribute,
            match_location: h.match_location,
            file: h.file,
            span: h.span,
        })
        .collect();
    let page_req = list_page(&params.pagination);
    clear_locations_for_summary(&mut enriched, page_req.summary, |item| {
        item.file = None;
        item.span = None;
    });
    let total = enriched.len();
    let (page, items) = page_list(enriched, page_req);
    json_result(&ItemsWithAttributeResponse {
        krate: params.crate_name,
        attribute_pattern: params.attribute_pattern,
        match_count: total,
        page,
        items,
    })
}

pub async fn function_signature(
    params: FunctionSignatureParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let (target_id, target_node) = snap
        .lookup_by_qualified_name(&params.target)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.target),
                None,
            )
        })?;
    let signature = snap
        .function_signature(target_id)
        .map_err(internal_error("function_signature"))?;
    json_result(&FunctionSignatureResponse {
        target: target_node.qualified_name,
        signature,
    })
}

pub async fn functions_with_filter(
    params: FunctionsWithFilterParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let (id, node) = snap
        .lookup_by_qualified_name(&params.krate)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.krate),
                None,
            )
        })?;
    let crate_id = match node.kind {
        NodeKind::Crate => id,
        NodeKind::Module => node.crate_id.or(node.parent_id).ok_or_else(|| {
            McpError::invalid_params(
                format!(
                    "`{}` resolves to a Module with no crate_id",
                    params.krate
                ),
                None,
            )
        })?,
        other => {
            return Err(McpError::invalid_params(
                format!(
                    "`{}` is a {other:?}, expected a crate or its root module",
                    params.krate
                ),
                None,
            ));
        }
    };

    let self_kind = match params.self_kind.as_deref() {
        None => None,
        Some("none") => Some(SelfKindFilter::None),
        Some("owned") => Some(SelfKindFilter::Owned),
        Some("ref") => Some(SelfKindFilter::Ref),
        Some("ref_mut") => Some(SelfKindFilter::RefMut),
        Some(other) => {
            return Err(McpError::invalid_params(
                format!(
                    "self_kind must be one of `none`, `owned`, `ref`, `ref_mut`; got `{other}`"
                ),
                None,
            ));
        }
    };
    let filter = FunctionFilter {
        min_param_count: params.min_param_count,
        has_param_type: params.has_param_type,
        returns_type_pattern: params.returns_type_pattern,
        is_async: params.is_async,
        self_kind,
    };

    let matches: Vec<FunctionWithSignature> = snap
        .functions_with_filter(crate_id, &filter)
        .map_err(internal_error("functions_with_filter"))?;

    // Pagination + summary mode (Item #4 + #5).
    // Slice in the wrapper layer; the query is workspace-bounded and not
    // inherently large — the cost is in serialization payload size.
    let total_match_count = matches.len();
    let offset = params.offset.unwrap_or(0);
    let limit = params.limit.unwrap_or(50);
    let summary = params.summary.unwrap_or(false);

    let sliced = matches
        .into_iter()
        .skip(offset)
        .take(limit);

    let enriched: Vec<FunctionsWithFilterMatch> = sliced
        .map(|m| FunctionsWithFilterMatch {
            target: m.qualified_name.clone(),
            qualified_name: m.qualified_name,
            signature: if summary { None } else { Some(m.signature) },
        })
        .collect();

    json_result(&FunctionsWithFilterResponse {
        krate: params.krate,
        total_match_count,
        offset,
        limit,
        match_count: enriched.len(),
        matches: enriched,
    })
}

pub async fn pub_use_pub_type_audit(
    params: PubUsePubTypeAuditParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let (id, node) = snap
        .lookup_by_qualified_name(&params.crate_name)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.crate_name),
                None,
            )
        })?;
    let crate_id = match node.kind {
        NodeKind::Crate => id,
        NodeKind::Module => node.crate_id.or(node.parent_id).ok_or_else(|| {
            McpError::invalid_params(
                format!(
                    "`{}` resolves to a Module with no crate_id",
                    params.crate_name
                ),
                None,
            )
        })?,
        other => {
            return Err(McpError::invalid_params(
                format!(
                    "`{}` is a {other:?}, expected a crate or its root module",
                    params.crate_name
                ),
                None,
            ));
        }
    };
    let findings: Vec<PubTypeAliasMasqueradingAsReexport> = snap
        .pub_use_pub_type_audit(crate_id)
        .map_err(internal_error("pub_use_pub_type_audit"))?;
    let mut enriched: Vec<EnrichedPubTypeAuditFinding> = {
        let rtxn = snap.read_txn().ok();
        findings
            .into_iter()
            .map(|f| {
                let pub_use_target_qualified = rtxn.as_ref().and_then(|t| {
                    snap.node_by_id(t, f.suspicious_pub_use_target_node_id)
                        .ok()
                        .flatten()
                        .map(|n| n.qualified_name)
                });
                EnrichedPubTypeAuditFinding {
                    alias_qualified_name: f.alias_qualified_name,
                    file: f.file,
                    span: f.span,
                    suspicious_pub_use_visible_name: f.suspicious_pub_use_visible_name,
                    suspicious_pub_use_target: pub_use_target_qualified,
                }
            })
            .collect()
    };
    let page_req = list_page(&params.pagination);
    clear_locations_for_summary(&mut enriched, page_req.summary, |finding| {
        finding.file = None;
        finding.span = None;
    });
    let finding_count = enriched.len();
    let (page, findings) = page_list(enriched, page_req);
    json_result(&PubUsePubTypeAuditResponse {
        krate: params.crate_name,
        finding_count,
        page,
        findings,
    })
}

pub async fn re_export_chain(
    params: ReExportChainParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let (target_id, target_node) = snap
        .lookup_by_qualified_name(&params.target)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.target),
                None,
            )
        })?;
    let chain: ReExportChain = snap
        .re_export_chain(target_id)
        .map_err(internal_error("re_export_chain"))?;
    let links: Vec<EnrichedReExportLink> = chain
        .links
        .into_iter()
        .map(|l| EnrichedReExportLink {
            from_module: l.from_module_qualified_name,
            visible_name: l.visible_name,
            depth: l.depth,
        })
        .collect();
    let link_count = links.len();
    let (page, links) = page_list(links, list_page(&params.pagination));
    json_result(&ReExportChainResponse {
        canonical: target_node.qualified_name,
        link_count,
        page,
        links,
    })
}

pub async fn overlaps(params: OverlapsParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let scope = parse_overlap_scope(params.scope.as_deref())?;
    let report: OverlapsReport = snap
        .overlaps_with_scope(scope)
        .map_err(internal_error("overlaps"))?;
    json_result(&report)
}

pub async fn missing_docs_audit(
    params: crate::tools::params::MissingDocsAuditParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;

    let crate_id_filter: Option<NodeId> = if let Some(qn) = &params.crate_name {
        let (id, node) = snap
            .lookup_by_qualified_name(qn)
            .map_err(internal_error("lookup_by_qualified_name"))?
            .ok_or_else(|| {
                McpError::invalid_params(
                    format!("no node found for qualified name `{qn}`"),
                    None,
                )
            })?;
        Some(match node.kind {
            NodeKind::Crate => id,
            NodeKind::Module => node.crate_id.or(node.parent_id).ok_or_else(|| {
                McpError::invalid_params(
                    format!("`{qn}` resolves to a Module with no crate_id"),
                    None,
                )
            })?,
            other => {
                return Err(McpError::invalid_params(
                    format!("`{qn}` is a {other:?}, expected a Crate or its root Module"),
                    None,
                ));
            }
        })
    } else {
        None
    };

    let kind_filter = match params.item_kind.as_deref() {
        None => crate::graph::docs_audit::default_kind_filter(),
        Some(labels) => {
            let mut set = std::collections::HashSet::new();
            for label in labels {
                let kind = parse_item_kind_filter(Some(label.as_str()))?
                    .ok_or_else(|| {
                        McpError::invalid_params(
                            format!("empty item_kind label in list"),
                            None,
                        )
                    })?;
                set.insert(kind);
            }
            set
        }
    };

    let opts = crate::graph::docs_audit::AuditOpts {
        crate_id_filter,
        kind_filter,
        skip_test_items: params.skip_test_items.unwrap_or(true),
    };

    let findings = crate::graph::docs_audit::missing_docs_audit(&snap, opts)
        .map_err(internal_error("missing_docs_audit"))?;

    #[derive(serde::Serialize)]
    struct ScopeSummary {
        directory: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        crate_name: Option<String>,
    }
    #[derive(serde::Serialize)]
    struct MissingDocsFindingRendered {
        target: String,
        qualified_name: String,
        item_kind: String,
        visibility: String,
        file: Option<String>,
        span: Option<(u32, u32)>,
    }
    #[derive(serde::Serialize)]
    struct Resp {
        scope: ScopeSummary,
        finding_count: usize,
        #[serde(flatten)]
        page: ListMeta,
        findings: Vec<MissingDocsFindingRendered>,
    }

    let mut rendered: Vec<MissingDocsFindingRendered> = findings
        .into_iter()
        .map(|f| MissingDocsFindingRendered {
            target: f.target.to_hex(),
            qualified_name: f.qualified_name,
            item_kind: item_kind_label(f.item_kind).to_string(),
            visibility: f.visibility,
            file: f.file,
            span: f.span,
        })
        .collect();
    let page_req = list_page(&params.pagination);
    clear_locations_for_summary(&mut rendered, page_req.summary, |finding| {
        finding.file = None;
        finding.span = None;
    });
    let finding_count = rendered.len();
    let (page, findings) = page_list(rendered, page_req);

    json_result(&Resp {
        scope: ScopeSummary {
            directory: params.directory,
            crate_name: params.crate_name,
        },
        finding_count,
        page,
        findings,
    })
}

pub async fn derive_audit(
    params: crate::tools::params::DeriveAuditParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;

    let crate_id_filter: Option<NodeId> = if let Some(qn) = &params.crate_name {
        let (id, node) = snap
            .lookup_by_qualified_name(qn)
            .map_err(internal_error("lookup_by_qualified_name"))?
            .ok_or_else(|| {
                McpError::invalid_params(
                    format!("no node found for qualified name `{qn}`"),
                    None,
                )
            })?;
        Some(match node.kind {
            NodeKind::Crate => id,
            NodeKind::Module => node.crate_id.or(node.parent_id).ok_or_else(|| {
                McpError::invalid_params(
                    format!("`{qn}` resolves to a Module with no crate_id"),
                    None,
                )
            })?,
            other => {
                return Err(McpError::invalid_params(
                    format!("`{qn}` is a {other:?}, expected a Crate or its root Module"),
                    None,
                ));
            }
        })
    } else {
        None
    };

    let kind_filter = match params.item_kind.as_deref() {
        None => crate::graph::derive_audit::default_kind_filter(),
        Some(labels) => {
            let mut set = std::collections::HashSet::new();
            for label in labels {
                let kind = parse_item_kind_filter(Some(label.as_str()))?
                    .ok_or_else(|| {
                        McpError::invalid_params(
                            "empty item_kind label in list".to_string(),
                            None,
                        )
                    })?;
                match kind {
                    ItemKind::Struct | ItemKind::Enum | ItemKind::Union => {}
                    other => {
                        return Err(McpError::invalid_params(
                            format!(
                                "derive_audit only accepts Struct | Enum | Union, got {other:?}"
                            ),
                            None,
                        ));
                    }
                }
                set.insert(kind);
            }
            set
        }
    };

    if params.required_derives.is_empty() {
        return Err(McpError::invalid_params(
            "required_derives must be a non-empty list of derive identifiers".to_string(),
            None,
        ));
    }
    let required_derives: std::collections::HashSet<String> =
        params.required_derives.iter().cloned().collect();

    let opts = crate::graph::derive_audit::AuditOpts {
        crate_id_filter,
        kind_filter,
        required_derives,
        pub_only: params.pub_only.unwrap_or(true),
        skip_test_items: params.skip_test_items.unwrap_or(true),
    };

    let findings = crate::graph::derive_audit::derive_audit(&snap, opts)
        .map_err(internal_error("derive_audit"))?;

    #[derive(serde::Serialize)]
    struct ScopeSummary {
        directory: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        crate_name: Option<String>,
    }
    #[derive(serde::Serialize)]
    struct DeriveFindingRendered {
        target: String,
        qualified_name: String,
        item_kind: String,
        visibility: String,
        file: Option<String>,
        span: Option<(u32, u32)>,
        current_derives: Vec<String>,
        missing_derives: Vec<String>,
    }
    #[derive(serde::Serialize)]
    struct Resp {
        scope: ScopeSummary,
        required_derives: Vec<String>,
        finding_count: usize,
        #[serde(flatten)]
        page: ListMeta,
        findings: Vec<DeriveFindingRendered>,
    }

    let mut rendered: Vec<DeriveFindingRendered> = findings
        .into_iter()
        .map(|f| DeriveFindingRendered {
            target: f.target.to_hex(),
            qualified_name: f.qualified_name,
            item_kind: item_kind_label(f.item_kind).to_string(),
            visibility: f.visibility,
            file: f.file,
            span: f.span,
            current_derives: f.current_derives,
            missing_derives: f.missing_derives,
        })
        .collect();
    let page_req = list_page(&params.pagination);
    clear_locations_for_summary(&mut rendered, page_req.summary, |finding| {
        finding.file = None;
        finding.span = None;
    });
    let finding_count = rendered.len();
    let (page, findings) = page_list(rendered, page_req);

    json_result(&Resp {
        scope: ScopeSummary {
            directory: params.directory,
            crate_name: params.crate_name,
        },
        required_derives: params.required_derives,
        finding_count,
        page,
        findings,
    })
}

// ----- helpers -----

fn enrich_dead_pub(snap: &OpenedSnapshot, f: DeadPubFinding) -> EnrichedDeadPub {
    let rtxn = snap.read_txn().ok();
    let visibility = match &rtxn {
        Some(t) => visibility_label(snap, t, &f.declared_visibility),
        None => "?".to_string(),
    };
    // Look up file/span for navigability — these live on the Item Node.
    let (file, span) = match &rtxn {
        Some(t) => match snap.node_by_id(t, f.target).ok().flatten() {
            Some(node) => (node.file, node.span),
            None => (None, None),
        },
        None => (None, None),
    };
    EnrichedDeadPub {
        qualified_name: f.qualified_name,
        item_kind: item_kind_label(f.item_kind),
        declared_visibility: visibility,
        file,
        span,
    }
}

fn enrich_crate_dead_pub(snap: &OpenedSnapshot, c: CrateDeadPub) -> EnrichedCrateDeadPub {
    EnrichedCrateDeadPub {
        krate: c.crate_qualified_name,
        findings: c
            .findings
            .into_iter()
            .map(|f| enrich_dead_pub(snap, f))
            .collect(),
    }
}

// ----- response shapes -----

#[derive(Debug, Serialize)]
pub(crate) struct DeadPubResponse {
    #[serde(rename = "crate")]
    pub(crate) krate: String,
    #[serde(flatten)]
    pub(crate) page: ListMeta,
    pub(crate) findings: Vec<EnrichedDeadPub>,
}

#[derive(Debug, Serialize)]
pub(crate) struct EnrichedDeadPub {
    pub(crate) qualified_name: String,
    pub(crate) item_kind: &'static str,
    pub(crate) declared_visibility: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) span: Option<(u32, u32)>,
}

#[derive(Debug, Serialize)]
pub(crate) struct DeadPubReportResponse {
    pub(crate) workspace: String,
    pub(crate) total_findings: usize,
    #[serde(flatten)]
    pub(crate) page: ListMeta,
    pub(crate) crates: Vec<EnrichedCrateDeadPub>,
}

#[derive(Debug, Serialize)]
pub(crate) struct EnrichedCrateDeadPub {
    #[serde(rename = "crate")]
    pub(crate) krate: String,
    pub(crate) findings: Vec<EnrichedDeadPub>,
}

#[derive(Debug, Serialize)]
pub(crate) struct EnumVariantsResponse {
    pub(crate) enum_qualified_name: String,
    pub(crate) variant_count: usize,
    #[serde(flatten)]
    pub(crate) page: ListMeta,
    pub(crate) variants: Vec<EnrichedEnumVariant>,
}

#[derive(Debug, Serialize)]
pub(crate) struct EnrichedEnumVariant {
    pub(crate) display_name: String,
    pub(crate) qualified_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) span: Option<(u32, u32)>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ItemAttributesResponse {
    pub(crate) target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) item_kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) span: Option<(u32, u32)>,
    pub(crate) attribute_count: usize,
    #[serde(flatten)]
    pub(crate) page: ListMeta,
    pub(crate) attributes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ItemsWithAttributeResponse {
    #[serde(rename = "crate")]
    pub(crate) krate: String,
    pub(crate) attribute_pattern: String,
    pub(crate) match_count: usize,
    #[serde(flatten)]
    pub(crate) page: ListMeta,
    pub(crate) items: Vec<EnrichedItemWithAttribute>,
}

#[derive(Debug, Serialize)]
pub(crate) struct EnrichedItemWithAttribute {
    pub(crate) qualified_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) item_kind: Option<&'static str>,
    pub(crate) matched_attribute: String,
    /// `"attr"` when the pattern matched the start of the attribute string,
    /// `"doc"` when it matched the start of a `///` doc-comment body.
    pub(crate) match_location: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) span: Option<(u32, u32)>,
}

#[derive(Debug, Serialize)]
pub(crate) struct PubUsePubTypeAuditResponse {
    #[serde(rename = "crate")]
    pub(crate) krate: String,
    pub(crate) finding_count: usize,
    #[serde(flatten)]
    pub(crate) page: ListMeta,
    pub(crate) findings: Vec<EnrichedPubTypeAuditFinding>,
}

#[derive(Debug, Serialize)]
pub(crate) struct EnrichedPubTypeAuditFinding {
    pub(crate) alias_qualified_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) span: Option<(u32, u32)>,
    pub(crate) suspicious_pub_use_visible_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) suspicious_pub_use_target: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ReExportChainResponse {
    pub(crate) canonical: String,
    pub(crate) link_count: usize,
    #[serde(flatten)]
    pub(crate) page: ListMeta,
    pub(crate) links: Vec<EnrichedReExportLink>,
}

#[derive(Debug, Serialize)]
pub(crate) struct EnrichedReExportLink {
    pub(crate) from_module: String,
    pub(crate) visible_name: String,
    pub(crate) depth: u8,
}

#[derive(Debug, Serialize)]
pub(crate) struct FunctionSignatureResponse {
    pub(crate) target: String,
    /// `None` when the target is not a function or extraction skipped it.
    pub(crate) signature: Option<FunctionSignature>,
}

#[derive(Debug, Serialize)]
pub(crate) struct FunctionsWithFilterResponse {
    #[serde(rename = "crate")]
    pub(crate) krate: String,
    /// Unfiltered total before `offset`/`limit` slicing — callers compare
    /// this to `offset + match_count` to detect "more pages exist".
    pub(crate) total_match_count: usize,
    /// Offset applied to the match list (after the filter, before the
    /// returned `matches`).
    pub(crate) offset: usize,
    /// Cap applied to the (offset-skipped) match list.
    pub(crate) limit: usize,
    /// Length of the returned `matches` (after offset+limit slicing). Always
    /// `<= limit`, and `<= total_match_count.saturating_sub(offset)`.
    pub(crate) match_count: usize,
    pub(crate) matches: Vec<FunctionsWithFilterMatch>,
}

#[derive(Debug, Serialize)]
pub(crate) struct FunctionsWithFilterMatch {
    /// Convenience alias for `qualified_name` so callers that want one
    /// "navigate-to" string don't have to know which field carries it.
    pub(crate) target: String,
    pub(crate) qualified_name: String,
    /// `None` when `summary=true` (the field is omitted entirely from the
    /// JSON response thanks to `skip_serializing_if`); otherwise carries the
    /// full FunctionSignature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) signature: Option<FunctionSignature>,
}
