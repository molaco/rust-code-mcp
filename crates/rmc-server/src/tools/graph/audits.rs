//! Workspace-audit endpoint family.
//!
//! Bridges between the MCP tool router and the graph-owned audit facade. Each
//! endpoint follows the shape documented in `graph_tools.rs`: parse MCP
//! parameters, call graph-owned audit entry points, paginate, serialize.
//!
//! Audits that need a full RA workspace load (`unsafe_audit`,
//! `channel_capacity_audit`, `fn_body_audit`) wrap the synchronous graph
//! facade call in `spawn_blocking` so the tokio runtime worker stays free.

use std::path::PathBuf;

use rmc_graph::graph::{
    ChannelCapacityAuditOptions, ChannelCapacityFinding, FnBodyAuditFinding, FnBodyAuditOptions,
    GraphAuditError, MutStaticAuditFinding, RecursionCheckOptions, RecursionCycle,
    UnsafeAuditFinding,
    run_channel_capacity_audit, run_fn_body_audit, run_mut_static_audit, run_recursion_check,
    run_unsafe_audit,
};
use crate::tools::graph::response::*;

use rmcp::{ErrorData as McpError, model::CallToolResult};

fn graph_audit_error(label: &'static str) -> impl FnOnce(anyhow::Error) -> McpError {
    move |error| {
        let message = format!("{error:#}");
        if error.downcast_ref::<GraphAuditError>().is_some() {
            McpError::invalid_params(message, None)
        } else {
            McpError::internal_error(format!("{label}: {message}"), None)
        }
    }
}

pub(crate) async fn unsafe_audit(
    params: crate::tools::params::UnsafeAuditParams,
) -> Result<CallToolResult, McpError> {
    let directory = PathBuf::from(&params.directory);
    let findings = tokio::task::spawn_blocking(move || run_unsafe_audit(&directory))
        .await
        .map_err(|e| McpError::internal_error(format!("spawn_blocking join error: {e}"), None))?
        .map_err(graph_audit_error("unsafe_audit"))?;

    #[derive(serde::Serialize)]
    struct Resp {
        directory: String,
        finding_count: usize,
        #[serde(flatten)]
        page: ListMeta,
        findings: Vec<UnsafeAuditFinding>,
    }
    let finding_count = findings.len();
    let (page, findings) = page_list(findings, list_page(&params.pagination));
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
    let directory = PathBuf::from(&params.directory);
    let findings = run_mut_static_audit(&directory)
        .map_err(graph_audit_error("mut_static_audit"))?;

    #[derive(serde::Serialize)]
    struct Resp {
        directory: String,
        finding_count: usize,
        #[serde(flatten)]
        page: ListMeta,
        findings: Vec<MutStaticAuditFinding>,
    }
    let mut rendered = findings;
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
    let directory = PathBuf::from(&params.directory);
    let output = run_recursion_check(
        &directory,
        RecursionCheckOptions {
            crate_name: params.crate_name.clone(),
            max_cycle_length: params.max_cycle_length,
        },
    )
    .map_err(graph_audit_error("recursion_check"))?;

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
        cycles: Vec<RecursionCycle>,
    }
    let cycle_count = output.cycles.len();
    let (page, cycles) = page_list(output.cycles, list_page(&params.pagination));

    json_result(&Resp {
        scope: ScopeSummary {
            directory: params.directory,
            crate_name: params.crate_name,
        },
        max_cycle_length: output.max_cycle_length,
        cycle_count,
        page,
        cycles,
    })
}

pub(crate) async fn channel_capacity_audit(
    params: crate::tools::params::ChannelCapacityAuditParams,
) -> Result<CallToolResult, McpError> {
    let directory = PathBuf::from(&params.directory);
    let crate_name = params.crate_name.clone();
    let skip_test_fns = params.skip_test_fns.unwrap_or(true);

    let findings = tokio::task::spawn_blocking(move || {
        run_channel_capacity_audit(
            &directory,
            ChannelCapacityAuditOptions {
                crate_name,
                skip_test_fns,
            },
        )
    })
        .await
        .map_err(|e| McpError::internal_error(format!("spawn_blocking join error: {e}"), None))?
        .map_err(graph_audit_error("channel_capacity_audit"))?;

    #[derive(serde::Serialize)]
    struct ScopeSummary {
        directory: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        crate_name: Option<String>,
    }
    #[derive(serde::Serialize)]
    struct Resp {
        scope: ScopeSummary,
        finding_count: usize,
        #[serde(flatten)]
        page: ListMeta,
        findings: Vec<ChannelCapacityFinding>,
    }

    let finding_count = findings.len();
    let (page, findings) = page_list(findings, list_page(&params.pagination));

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
    let directory = PathBuf::from(&params.directory);
    let crate_name = params.crate_name.clone();
    let patterns = params.patterns.clone();
    let skip_test_fns = params.skip_test_fns.unwrap_or(true);

    let output = tokio::task::spawn_blocking(move || {
        run_fn_body_audit(
            &directory,
            FnBodyAuditOptions {
                crate_name,
                patterns,
                skip_test_fns,
            },
        )
    })
        .await
        .map_err(|e| McpError::internal_error(format!("spawn_blocking join error: {e}"), None))?
        .map_err(graph_audit_error("fn_body_audit"))?;

    #[derive(serde::Serialize)]
    struct ScopeSummary {
        directory: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        crate_name: Option<String>,
    }
    #[derive(serde::Serialize)]
    struct Resp {
        scope: ScopeSummary,
        patterns_used: Vec<String>,
        finding_count: usize,
        #[serde(flatten)]
        page: ListMeta,
        findings: Vec<FnBodyAuditFinding>,
    }

    let finding_count = output.findings.len();
    let (page, findings) = page_list(output.findings, list_page(&params.pagination));

    json_result(&Resp {
        scope: ScopeSummary {
            directory: params.directory,
            crate_name: params.crate_name,
        },
        patterns_used: output.patterns_used,
        finding_count,
        page,
        findings,
    })
}
