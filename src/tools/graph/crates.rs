//! Crate-level endpoint family.
//!
//! Endpoints that operate at the workspace's crate granularity — listing the
//! crate→crate edge set, computing Martin instability/abstractness metrics
//! per crate, and checking user-supplied forbidden-dependency rules. Each
//! endpoint follows the shape documented in `graph_tools.rs`: resolve
//! directory, open snapshot, run the query, serialize.

use serde::Serialize;

use crate::graph::{CrateEdge, CrateMetric, ForbiddenDependencyViolation};
use crate::tools::graph::response::*;
use crate::tools::params::{
    CrateDependencyMetricParams, CrateEdgesParams, ForbiddenDependencyCheckParams,
};

use rmcp::{ErrorData as McpError, model::CallToolResult};

pub(crate) async fn crate_edges(params: CrateEdgesParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let edges: Vec<CrateEdge> = snap
        .crate_edges()
        .map_err(internal_error("crate_edges"))?;
    let (page, edges) = page_list(edges, list_page(&params.pagination));
    json_result(&CrateEdgesResponse { page, edges })
}

pub(crate) async fn forbidden_dependency_check(
    params: ForbiddenDependencyCheckParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let rule_count = params.rules.len();
    let violations: Vec<ForbiddenDependencyViolation> = snap
        .forbidden_dependency_check(&params.rules)
        .map_err(internal_error("forbidden_dependency_check"))?;
    let violation_count = violations.len();
    let (page, violations) = page_list(violations, list_page(&params.pagination));
    json_result(&ForbiddenDependencyCheckResponse {
        rule_count,
        violation_count,
        page,
        violations,
    })
}

pub(crate) async fn crate_dependency_metric(
    params: CrateDependencyMetricParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let mut metrics: Vec<CrateMetric> = snap
        .crate_dependency_metric()
        .map_err(internal_error("crate_dependency_metric"))?;

    // Item #7: optional sort + top_n slicing. Sort first, then slice.
    if let Some(sort_key) = params.sort_by.as_deref() {
        match sort_key {
            "instability" => metrics.sort_by(|a, b| {
                b.instability
                    .partial_cmp(&a.instability)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
            "abstractness" => metrics.sort_by(|a, b| {
                b.abstractness
                    .partial_cmp(&a.abstractness)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
            "item_count" => metrics.sort_by(|a, b| b.item_count.cmp(&a.item_count)),
            "afferent" => metrics.sort_by(|a, b| b.afferent.cmp(&a.afferent)),
            "efferent" => metrics.sort_by(|a, b| b.efferent.cmp(&a.efferent)),
            other => {
                return Err(McpError::invalid_params(
                    format!(
                        "sort_by must be one of `instability`, `item_count`, `afferent`, `efferent`, `abstractness`; got `{other}`"
                    ),
                    None,
                ));
            }
        }
    }

    // Render NodeIds as hex strings rather than the raw 32-byte arrays
    // serde_bytes_32 emits for [u8; 32].
    let rendered: Vec<CrateMetricRendered> = metrics
        .into_iter()
        .map(|m| CrateMetricRendered {
            crate_id: m.crate_id.to_hex(),
            crate_name: m.crate_name,
            efferent: m.efferent,
            afferent: m.afferent,
            instability: m.instability,
            abstractness: m.abstractness,
            item_count: m.item_count,
        })
        .collect();
    let mut page_req = list_page(&params.pagination);
    if let Some(n) = params.top_n {
        page_req.limit = n;
    }
    let (page, metrics) = page_list(rendered, page_req);
    let crate_count = metrics.len();
    json_result(&CrateDependencyMetricResponse {
        crate_count,
        page,
        metrics,
    })
}

// ----- response shapes -----

#[derive(Debug, Serialize)]
pub(crate) struct CrateEdgesResponse {
    #[serde(flatten)]
    pub(crate) page: ListMeta,
    pub(crate) edges: Vec<CrateEdge>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ForbiddenDependencyCheckResponse {
    pub(crate) rule_count: usize,
    pub(crate) violation_count: usize,
    #[serde(flatten)]
    pub(crate) page: ListMeta,
    pub(crate) violations: Vec<ForbiddenDependencyViolation>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CrateDependencyMetricResponse {
    pub(crate) crate_count: usize,
    #[serde(flatten)]
    pub(crate) page: ListMeta,
    pub(crate) metrics: Vec<CrateMetricRendered>,
}

/// MCP-rendered mirror of `CrateMetric`: emits `crate_id` as a 64-char hex
/// string instead of the raw 32-byte array `serde_bytes_32` would produce
/// for `NodeId`.
#[derive(Debug, Serialize)]
pub(crate) struct CrateMetricRendered {
    pub(crate) crate_id: String,
    pub(crate) crate_name: String,
    pub(crate) efferent: u32,
    pub(crate) afferent: u32,
    pub(crate) instability: f64,
    pub(crate) abstractness: f64,
    pub(crate) item_count: u32,
}
