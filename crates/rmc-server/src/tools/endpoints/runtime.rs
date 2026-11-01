//! Runtime lifecycle status and cleanup tools.

use rmcp::{
    ErrorData as McpError,
    model::{CallToolResult, Content},
    schemars,
};

use crate::mcp::{RuntimeClearRequest, RuntimeClearScope, RuntimeState};

#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct RuntimeStatusParams {}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct ClearRuntimeParams {
    #[schemars(
        description = "all (default) | workspace | semantic_only | search_cache_only | sync_tracking_only."
    )]
    #[serde(default)]
    pub scope: Option<RuntimeClearScope>,
    #[schemars(
        description = "Optional workspace path. With scope=workspace this is required; with semantic_only/search_cache_only/sync_tracking_only it limits cleanup to one workspace."
    )]
    #[serde(default)]
    pub workspace: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct AnalysisMemoryParams {
    #[schemars(
        description = "Workspace root of an analysis already loaded — a path runtime_status lists under `semantic`."
    )]
    pub directory: String,
    #[schemars(
        description = "How many ingredients to return, largest first. Default 25; 0 returns all 90-odd."
    )]
    #[serde(default)]
    pub top: Option<usize>,
}

/// Report what one loaded analysis is holding, ingredient by ingredient.
pub(crate) async fn analysis_memory(
    runtime: &RuntimeState,
    params: AnalysisMemoryParams,
) -> Result<CallToolResult, McpError> {
    let directory = crate::tools::paths::require_absolute("directory", &params.directory)?;
    let top = params.top.unwrap_or(25);

    // Through the same door as every other rust-analyzer call: the walk that
    // computes heap sizes descends the shape of parsed source, and a tokio
    // worker's 2 MiB stack is what took this server down with an `abort` the
    // last time such work ran on one.
    let report = crate::deep_stack::with_semantic(&runtime.semantic(), "analysis_memory", {
        let directory = directory.clone();
        move |service| {
            service
                .memory_breakdown(&directory, top)
                .map_err(|error| McpError::invalid_params(error.to_string(), None))
        }
    })
    .await?;

    let body = serde_json::to_string_pretty(&report).map_err(|e| {
        McpError::internal_error(format!("Failed to serialize memory report: {}", e), None)
    })?;
    Ok(CallToolResult::success(vec![Content::text(body)]))
}

pub(crate) async fn runtime_status(
    runtime: &RuntimeState,
    _params: RuntimeStatusParams,
) -> Result<CallToolResult, McpError> {
    let status = runtime.status().await;
    let body = serde_json::to_string_pretty(&status)
        .map_err(|e| McpError::internal_error(format!("Failed to serialize status: {}", e), None))?;
    Ok(CallToolResult::success(vec![Content::text(body)]))
}

pub(crate) async fn clear_runtime(
    runtime: &RuntimeState,
    params: ClearRuntimeParams,
) -> Result<CallToolResult, McpError> {
    let scope = params.scope.unwrap_or_default();
    let workspace = params.workspace.map(std::path::PathBuf::from);
    if scope == RuntimeClearScope::Workspace && workspace.is_none() {
        return Err(McpError::invalid_params(
            "workspace is required when scope is 'workspace'",
            None,
        ));
    }

    let report = runtime
        .clear(RuntimeClearRequest { scope, workspace })
        .await;
    let body = serde_json::to_string_pretty(&report)
        .map_err(|e| McpError::internal_error(format!("Failed to serialize clear report: {}", e), None))?;
    Ok(CallToolResult::success(vec![Content::text(body)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refusal is the feature, not a limitation: loading the project to
    /// answer would report a database built for the question — a cold start —
    /// and the output would look exactly like a live daemon's.
    ///
    /// Two assertions, because only the second one can fail silently: the
    /// message names what happened, and the project count proves nothing was
    /// loaded on the way to saying it.
    #[tokio::test]
    async fn analysis_memory_will_not_load_a_project_to_answer() {
        let runtime = RuntimeState::standalone();
        let project = tempfile::tempdir().expect("tempdir");

        let error = analysis_memory(
            &runtime,
            AnalysisMemoryParams {
                directory: project.path().display().to_string(),
                top: None,
            },
        )
        .await
        .expect_err("an unloaded project has no memory report to give");

        assert!(
            error.to_string().contains("no analysis is loaded"),
            "unexpected error: {error}"
        );
        assert_eq!(
            runtime.status().await.semantic.project_count,
            0,
            "asking about an unloaded project loaded it — the report now describes a database \
             built to answer the question instead of the daemon"
        );
    }

    #[tokio::test]
    async fn runtime_clear_workspace_requires_workspace_param() {
        let runtime = RuntimeState::standalone();

        let error = clear_runtime(
            &runtime,
            ClearRuntimeParams {
                scope: Some(RuntimeClearScope::Workspace),
                workspace: None,
            },
        )
        .await
        .expect_err("workspace scope should require workspace");

        assert!(error.to_string().contains("workspace is required"));
    }
}
