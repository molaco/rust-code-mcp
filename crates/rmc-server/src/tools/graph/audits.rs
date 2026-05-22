//! Workspace-audit endpoint family.
//!
//! Bridges between the MCP tool router and the algorithmic audit cores in
//! `rmc_graph::graph::*_audit` modules. Each endpoint follows the shape
//! documented in `graph_tools.rs`: resolve directory, open snapshot, resolve
//! optional crate filter, run the audit, render NodeIds as hex, serialize.
//!
//! Audits that need a full RA workspace load (`unsafe_audit`,
//! `channel_capacity_audit`, `fn_body_audit`) wrap the synchronous loader +
//! audit call in `spawn_blocking` so the tokio runtime worker stays free.

use rmc_graph::graph::{NodeId, NodeKind};
use crate::tools::graph::response::*;

use rmcp::{ErrorData as McpError, model::CallToolResult};

pub(crate) async fn unsafe_audit(
    params: crate::tools::params::UnsafeAuditParams,
) -> Result<CallToolResult, McpError> {
    let directory = params.directory.clone();
    // The audit calls `loader::load` (full RA workspace load, ~2-3s) and
    // then walks every file's syntax tree. Run on a blocking thread so the
    // tokio runtime worker stays free for concurrent tool calls.
    let findings: Vec<rmc_graph::graph::unsafe_audit::UnsafeFinding> =
        tokio::task::spawn_blocking(move || -> Result<_, McpError> {
            let snap = open_workspace_snapshot(&directory)?;
            let canonical = std::path::PathBuf::from(&directory)
                .canonicalize()
                .map_err(|e| McpError::invalid_params(format!("canonicalize: {e}"), None))?;
            let loaded = rmc_graph::graph::loader::load(&canonical)
                .map_err(internal_error("loader::load"))?;
            snap.unsafe_audit(&loaded)
                .map_err(internal_error("unsafe_audit"))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("spawn_blocking join error: {e}"), None))??;
    // Render NodeIds as hex strings rather than the raw 32-byte arrays
    // serde_bytes_32 emits for [u8; 32].
    #[derive(serde::Serialize)]
    struct UnsafeFindingRendered {
        file: String,
        span: (u32, u32),
        line_count: u32,
        enclosing_function: Option<String>,
        enclosing_function_name: Option<String>,
        has_safety_comment: bool,
    }
    #[derive(serde::Serialize)]
    struct Resp {
        directory: String,
        finding_count: usize,
        #[serde(flatten)]
        page: ListMeta,
        findings: Vec<UnsafeFindingRendered>,
    }
    let rendered: Vec<UnsafeFindingRendered> = findings
        .into_iter()
        .map(|f| UnsafeFindingRendered {
            file: f.file,
            span: f.span,
            line_count: f.line_count,
            enclosing_function: f.enclosing_function.map(|n| n.to_hex()),
            enclosing_function_name: f.enclosing_function_name,
            has_safety_comment: f.has_safety_comment,
        })
        .collect();
    let finding_count = rendered.len();
    let (page, findings) = page_list(rendered, list_page(&params.pagination));
    json_result(&Resp {
        directory: params.directory,
        finding_count,
        page,
        findings,
    })
}

pub(crate) async fn mut_static_audit(
    params: crate::tools::params::MutStaticAuditParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let findings = snap
        .mut_static_audit()
        .map_err(internal_error("mut_static_audit"))?;
    // Render NodeIds as hex strings rather than the raw 32-byte arrays
    // serde_bytes_32 emits for [u8; 32].
    #[derive(serde::Serialize)]
    struct MutStaticFindingRendered {
        item: String,
        qualified_name: String,
        matched_pattern: String,
        type_string: String,
        file: Option<String>,
        span: Option<(u32, u32)>,
    }
    #[derive(serde::Serialize)]
    struct Resp {
        directory: String,
        finding_count: usize,
        #[serde(flatten)]
        page: ListMeta,
        findings: Vec<MutStaticFindingRendered>,
    }
    let mut rendered: Vec<MutStaticFindingRendered> = findings
        .into_iter()
        .map(|f| MutStaticFindingRendered {
            item: f.item.to_hex(),
            qualified_name: f.qualified_name,
            matched_pattern: f.matched_pattern,
            type_string: f.type_string,
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
        directory: params.directory,
        finding_count,
        page,
        findings,
    })
}

pub(crate) async fn recursion_check(
    params: crate::tools::params::RecursionCheckParams,
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

    let max_cycle_length =
        rmc_graph::graph::recursion_check::clamp_cycle_length(params.max_cycle_length);

    let opts = rmc_graph::graph::recursion_check::RecursionOpts {
        crate_id_filter,
        max_cycle_length,
    };

    let cycles = rmc_graph::graph::recursion_check::recursion_check(&snap, opts)
        .map_err(internal_error("recursion_check"))?;

    let mut rendered: Vec<RecursionCycleRendered> = Vec::with_capacity(cycles.len());
    for cycle in cycles {
        let qualified_names =
            rmc_graph::graph::recursion_check::enclosing_fn_qualified_names(&snap, &cycle.fns)
                .map_err(internal_error("enclosing_fn_qualified_names"))?;
        let starting_node_id = cycle
            .fns
            .first()
            .map(|id| id.to_hex())
            .unwrap_or_default();
        rendered.push(RecursionCycleRendered {
            fns: qualified_names,
            cycle_length: cycle.cycle_length,
            direct_recursion: cycle.direct_recursion,
            starting_node_id,
        });
    }

    #[derive(serde::Serialize)]
    struct ScopeSummary {
        directory: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        crate_name: Option<String>,
    }
    #[derive(serde::Serialize)]
    struct Resp {
        scope: ScopeSummary,
        max_cycle_length: usize,
        cycle_count: usize,
        #[serde(flatten)]
        page: ListMeta,
        cycles: Vec<RecursionCycleRendered>,
    }
    let cycle_count = rendered.len();
    let (page, cycles) = page_list(rendered, list_page(&params.pagination));

    json_result(&Resp {
        scope: ScopeSummary {
            directory: params.directory,
            crate_name: params.crate_name,
        },
        max_cycle_length,
        cycle_count,
        page,
        cycles,
    })
}

#[derive(serde::Serialize)]
pub(crate) struct RecursionCycleRendered {
    pub(crate) fns: Vec<String>,
    pub(crate) cycle_length: usize,
    pub(crate) direct_recursion: bool,
    pub(crate) starting_node_id: String,
}

pub(crate) async fn channel_capacity_audit(
    params: crate::tools::params::ChannelCapacityAuditParams,
) -> Result<CallToolResult, McpError> {
    let directory = params.directory.clone();
    let crate_name = params.crate_name.clone();
    let skip_test_fns = params.skip_test_fns.unwrap_or(true);

    let findings: Vec<rmc_graph::graph::channel_audit::ChannelFinding> =
        tokio::task::spawn_blocking(move || -> Result<_, McpError> {
            let snap = open_workspace_snapshot(&directory)?;

            let crate_id_filter: Option<NodeId> = if let Some(qn) = &crate_name {
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

            let canonical = std::path::PathBuf::from(&directory)
                .canonicalize()
                .map_err(|e| McpError::invalid_params(format!("canonicalize: {e}"), None))?;
            let loaded = rmc_graph::graph::loader::load(&canonical)
                .map_err(internal_error("loader::load"))?;

            let opts = rmc_graph::graph::channel_audit::ChannelAuditOpts {
                crate_id_filter,
                skip_test_fns,
            };
            rmc_graph::graph::channel_audit::channel_capacity_audit(&loaded, &snap, opts)
                .map_err(internal_error("channel_capacity_audit"))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("spawn_blocking join error: {e}"), None))??;

    #[derive(serde::Serialize)]
    struct ScopeSummary {
        directory: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        crate_name: Option<String>,
    }
    #[derive(serde::Serialize)]
    struct ChannelFindingRendered {
        crate_name: String,
        kind: String,
        bounded: bool,
        capacity: Option<u64>,
        file: String,
        span: (u32, u32),
        enclosing_function: Option<String>,
        enclosing_function_name: Option<String>,
    }
    #[derive(serde::Serialize)]
    struct Resp {
        scope: ScopeSummary,
        finding_count: usize,
        #[serde(flatten)]
        page: ListMeta,
        findings: Vec<ChannelFindingRendered>,
    }

    let rendered: Vec<ChannelFindingRendered> = findings
        .into_iter()
        .map(|f| ChannelFindingRendered {
            crate_name: f.crate_name,
            kind: f.kind,
            bounded: f.bounded,
            capacity: f.capacity,
            file: f.file,
            span: f.span,
            enclosing_function: f.enclosing_function.map(|n| n.to_hex()),
            enclosing_function_name: f.enclosing_function_name,
        })
        .collect();
    let finding_count = rendered.len();
    let (page, findings) = page_list(rendered, list_page(&params.pagination));

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

pub(crate) async fn fn_body_audit(
    params: crate::tools::params::FnBodyAuditParams,
) -> Result<CallToolResult, McpError> {
    let directory = params.directory.clone();
    let crate_name = params.crate_name.clone();
    let patterns_input = params.patterns.clone();
    let skip_test_fns = params.skip_test_fns.unwrap_or(true);

    let patterns_set =
        rmc_graph::graph::fn_body_audit::parse_pattern_filter(patterns_input.as_deref())
            .map_err(|m| McpError::invalid_params(m, None))?;

    let mut patterns_used: Vec<String> =
        patterns_set.iter().map(|s| s.to_string()).collect();
    patterns_used.sort();

    let findings: Vec<rmc_graph::graph::fn_body_audit::FnBodyFinding> =
        tokio::task::spawn_blocking(move || -> Result<_, McpError> {
            let snap = open_workspace_snapshot(&directory)?;

            let crate_id_filter: Option<NodeId> = if let Some(qn) = &crate_name {
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

            let canonical = std::path::PathBuf::from(&directory)
                .canonicalize()
                .map_err(|e| McpError::invalid_params(format!("canonicalize: {e}"), None))?;
            let loaded = rmc_graph::graph::loader::load(&canonical)
                .map_err(internal_error("loader::load"))?;

            let opts = rmc_graph::graph::fn_body_audit::FnBodyAuditOpts {
                crate_id_filter,
                patterns: patterns_set,
                skip_test_fns,
            };
            rmc_graph::graph::fn_body_audit::fn_body_audit(&loaded, &snap, opts)
                .map_err(internal_error("fn_body_audit"))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("spawn_blocking join error: {e}"), None))??;

    #[derive(serde::Serialize)]
    struct ScopeSummary {
        directory: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        crate_name: Option<String>,
    }
    #[derive(serde::Serialize)]
    struct FnBodyFindingRendered {
        target: Option<String>,
        qualified_name: Option<String>,
        pattern: String,
        file: String,
        span: (u32, u32),
        context: String,
    }
    #[derive(serde::Serialize)]
    struct Resp {
        scope: ScopeSummary,
        patterns_used: Vec<String>,
        finding_count: usize,
        #[serde(flatten)]
        page: ListMeta,
        findings: Vec<FnBodyFindingRendered>,
    }

    let rendered: Vec<FnBodyFindingRendered> = findings
        .into_iter()
        .map(|f| FnBodyFindingRendered {
            target: f.target.map(|n| n.to_hex()),
            qualified_name: f.qualified_name,
            pattern: f.pattern,
            file: f.file,
            span: f.span,
            context: f.context,
        })
        .collect();
    let finding_count = rendered.len();
    let (page, findings) = page_list(rendered, list_page(&params.pagination));

    json_result(&Resp {
        scope: ScopeSummary {
            directory: params.directory,
            crate_name: params.crate_name,
        },
        patterns_used,
        finding_count,
        page,
        findings,
    })
}
